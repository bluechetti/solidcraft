//! Curvature-continuous patches round an irregular corner of a bicubic patch network: a corner
//! where `n` other than four faces meet (Loop and Schaefer, *G2 Tensor Product Splines over
//! Extraordinary Vertices*, Symposium on Geometry Processing 2008).
//!
//! A network whose points are a bicubic B-spline's control points gives faces that meet in
//! curvature wherever four faces meet at each corner. Where `n != 4` faces meet, bicubic faces
//! cannot: there the `n` faces round the corner are each a Bézier patch of degree 7 both ways,
//! joined to one another and to the bicubic faces round them in G2. Every point of those patches
//! is a fixed blend of the corner's *two-ring* of network points, worked out once for each `n`
//! ([`CornerBasis::for_faces`]): the least *bicubic energy* (the integral of the squares of the
//! fourth derivatives along u and along v, nothing for a bicubic patch) under the linear
//! conditions of G2, so that at `n = 4` the patches are the bicubic B-spline's own.
//!
//! Upstream: none (not upstream; solidcore-native). The construction is the paper's; this file
//! follows `SurfaceCraft`'s `surfaces/g2_corner.rs` (this fork's author's own earlier app, not
//! d0pawlus/SurfaceCraft), with the solve changed as
//! under Deviations.
//!
//! | Rust                         | Upstream |
//! |------------------------------|----------|
//! | [`CornerBasis`]              | none     |
//! | [`CornerBasis::for_faces`]   | none     |
//! | [`ring_index`]               | none     |
//!
//! **The two-ring**, numbered as [`CornerBasis`]'s blends read it: the corner itself (0), then
//! for each sector `i` (the face between spokes `i` and `i + 1`, counter-clockwise) six points
//! ([`SECTOR`]): `a10` the spoke's end, `a11` the face's far corner, `a20` the next point out
//! along the spoke, `a21` beside it, `a12` beyond `a11` across from the spoke and `a22`
//! diagonally out. Patch `i` has the corner at (0, 0), runs along spoke `i` in u and spoke
//! `i + 1` in v, so that `P_i(0, t) = P_{i+1}(t, 0)`; its point (a, b) is the `a + 8 b`-th.
//!
//! **The conditions** are the paper's (equations 5 to 10, with the maps `phi_n` and `psi_n` of
//! its Sections 3.1 and 3.2). Each is a polynomial identity along an edge, of degree under 14,
//! and is held at 14 places along that edge: a polynomial of degree under 14 that vanishes at 14
//! places is nothing, so each condition holds along the whole edge, not only where it was
//! asked. That is an identity on the patches' points, not a sample of the surface.
//!
//! What is certified, and what is not:
//! - **The conditions hold of the blends** returned, each to [`HELD`] of a row scaled to unit
//!   length: [`CornerBasis::for_faces`] checks every row against every two-ring point and
//!   refuses otherwise.
//! - **The patches are the definition.** They are polynomial and given exactly by the two-ring,
//!   so there is no net approximating something else, and no bound to prove.
//! - **Shape is not certified.** The paper names slightly negative weights and, at three faces,
//!   hot spots of Gaussian curvature. A caller that needs fair highlight lines checks them.
//!
//! Deviations (from `SurfaceCraft`'s file): the independent conditions and the space they leave
//! free are found by a Householder factorisation with column pivoting, not a singular value
//! decomposition, so the kernel needs no linear algebra crate; the least energy in that space is
//! one and the same patch whatever basis spans it, so the blends agree to rounding. A basis is
//! returned to its caller and not kept in a global table: the kernel has no global state. The
//! paper's extra conditions that keep four of the blends to two faces each (its Section 7.1) are
//! not imposed, as in `SurfaceCraft`.

use crate::error::{Result, SolidError};
use crate::math::Point3;

const OP: &str = "nurbs::g2_corner";

/// The six two-ring points of a sector, after the corner: `(j, k)` is the paper's `a^{jk}`, `j`
/// steps out along the spoke and `k` across.
pub const SECTOR: [(u8, u8); 6] = [(1, 0), (1, 1), (2, 0), (2, 1), (1, 2), (2, 2)];

/// The degree of the patches each way.
pub const DEGREE: usize = 7;
/// The points of one patch.
pub const POINTS: usize = (DEGREE + 1) * (DEGREE + 1);

/// The fewest faces at a corner a basis is made for.
pub const MIN_FACES: usize = 3;
/// The most faces at a corner a basis is made for.
pub const MAX_FACES: usize = 12;

/// How nearly every condition holds of the blends returned, on rows scaled to unit length.
pub const HELD: f64 = 1.0e-8;

/// A remaining row shorter than this, against the longest, says nothing new: the conditions
/// repeat one another at each corner of a patch (the paper counts the repeats).
const REPEATS: f64 = 1.0e-9;

/// A pivot of the energy's factorisation under this, against its largest, means the energy does
/// not fix the patches.
const UNFIXED: f64 = 1.0e-12;

/// The places along an edge each condition is held at: more than any of them has degree.
const PLACES: usize = 14;

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

/// The blends for a corner of `n` faces: patch `i`'s point `k` is the sum over the `1 + 6 n`
/// two-ring points of `weight * two_ring[c]`.
#[derive(Clone, Debug)]
pub struct CornerBasis {
    faces: usize,
    /// How many of the conditions say something the others do not.
    conditions: usize,
    /// The shortest condition taken and the longest left out, each against the longest of all.
    separation: (f64, f64),
    /// `weights[(i * POINTS + k) * columns + c]`, `columns` = `1 + 6 n`.
    weights: Vec<f64>,
}

/// The number of a two-ring point: the corner (`jk` = (0, 0)), or sector `sector`'s point
/// `(j, k)` (sectors taken round, so `sector` may be any integer).
///
/// # Errors
///
/// `InvalidInput` when `n` is nothing or `jk` is no point of the two-ring.
pub fn ring_index(n: usize, sector: i64, jk: (u8, u8)) -> Result<usize> {
    if n == 0 {
        return Err(bad("a corner of no faces"));
    }
    if jk == (0, 0) {
        return Ok(0);
    }
    let round = i64::try_from(n).map_err(|_| bad("too many faces"))?;
    let i = usize::try_from(sector.rem_euclid(round)).map_err(|_| bad("too many faces"))?;
    let place = SECTOR
        .iter()
        .position(|&s| s == jk)
        .ok_or_else(|| bad("no point of the two-ring"))?;
    Ok(1 + 6 * i + place)
}

impl CornerBasis {
    /// The faces at the corner.
    #[must_use]
    pub const fn faces(&self) -> usize {
        self.faces
    }

    /// How many two-ring points the blends are over: `1 + 6 n`.
    #[must_use]
    pub const fn columns(&self) -> usize {
        1 + 6 * self.faces
    }

    /// How many of the G2 conditions say something the others do not. The paper writes `55 n`
    /// conditions on the patches' `64 n` points, counted a patch and an edge at a time; taken
    /// together six of them follow from the rest (more at 3, 4 and 6 faces, where the spokes'
    /// cosine is a half, nothing or minus a half), so this is `55 n - 6` at 5 faces and at 7 or
    /// more.
    #[must_use]
    pub const fn conditions(&self) -> usize {
        self.conditions
    }

    /// How clearly the conditions that say something new were told from those that repeat the
    /// others: the shortest row taken and the longest row left out, each against the longest row
    /// of all, once the rows taken before are reflected away. They are many orders apart.
    #[must_use]
    pub const fn separation(&self) -> (f64, f64) {
        self.separation
    }

    /// The blend of patch `patch`'s point `point`: a weight for each two-ring point.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a patch or a point the corner does not have.
    pub fn blend(&self, patch: usize, point: usize) -> Result<&[f64]> {
        if patch >= self.faces || point >= POINTS {
            return Err(bad("no such patch or point at this corner"));
        }
        let c = self.columns();
        let at = (patch * POINTS + point) * c;
        within(self.weights.get(at..at + c))
    }

    /// The `n` patches' points ([`POINTS`] each, patch after patch) for these two-ring points.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when `ring` does not hold `1 + 6 n` points.
    pub fn patches(&self, ring: &[Point3]) -> Result<Vec<Point3>> {
        let c = self.columns();
        if ring.len() != c {
            return Err(bad("a two-ring holds 1 + 6 n points"));
        }
        Ok(self
            .weights
            .chunks_exact(c)
            .map(|blend| {
                let mut p = Point3::new(0.0, 0.0, 0.0);
                for (w, q) in blend.iter().zip(ring) {
                    p.x += w * q.x;
                    p.y += w * q.y;
                    p.z += w * q.z;
                }
                p
            })
            .collect())
    }

    /// The place (u, v) on patch `patch` as a blend of the two-ring: a weight for each two-ring
    /// point.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for a patch the corner does not have.
    pub fn place_blend(&self, patch: usize, u: f64, v: f64) -> Result<Vec<f64>> {
        let (bu, bv) = (bernstein(DEGREE, u, 0), bernstein(DEGREE, v, 0));
        let mut out = vec![0.0; self.columns()];
        for (b, wv) in bv.iter().enumerate() {
            for (a, wu) in bu.iter().enumerate() {
                let w = wu * wv;
                for (o, x) in out.iter_mut().zip(self.blend(patch, a + (DEGREE + 1) * b)?) {
                    *o += w * x;
                }
            }
        }
        Ok(out)
    }

    /// The basis for a corner of `n` faces. The work grows with the cube of `n` (a fraction of a
    /// second at five faces): a caller that meets many corners keeps one basis for each `n`.
    ///
    /// # Errors
    ///
    /// `InvalidInput` for `n` outside [`MIN_FACES`] to [`MAX_FACES`]; `PortInvariant` when the
    /// energy does not fix the patches or a condition is not held to [`HELD`] (neither happens
    /// for a supported `n`; a wrongly written condition would show here).
    pub fn for_faces(n: usize) -> Result<Self> {
        if !(MIN_FACES..=MAX_FACES).contains(&n) {
            return Err(bad("a basis is made for corners of 3 to 12 faces"));
        }
        solve(n)
    }
}

// ---- Polynomials ---------------------------------------------------------------------------

/// The Bernstein polynomials of degree `n` at `t`, or their `r`-th derivatives.
fn bernstein(n: usize, t: f64, r: usize) -> Vec<f64> {
    if r > n {
        return vec![0.0; n + 1];
    }
    let mut b = vec![1.0];
    // The values at degree n - r, by the de Casteljau recurrence.
    for _ in 0..n - r {
        b = (0..=b.len())
            .map(|i| {
                let left = i
                    .checked_sub(1)
                    .and_then(|j| b.get(j))
                    .copied()
                    .unwrap_or(0.0);
                let right = b.get(i).copied().unwrap_or(0.0);
                left * t + right * (1.0 - t)
            })
            .collect();
    }
    // Each derivative raises the degree by one: B'(i, d) = d (B(i - 1, d - 1) - B(i, d - 1)).
    for d in n - r + 1..=n {
        b = (0..=b.len())
            .map(|i| {
                let left = i
                    .checked_sub(1)
                    .and_then(|j| b.get(j))
                    .copied()
                    .unwrap_or(0.0);
                let right = b.get(i).copied().unwrap_or(0.0);
                d as f64 * (left - right)
            })
            .collect();
    }
    b
}

/// The uniform cubic B-spline's four basis functions over its middle span, or their `r`-th
/// derivatives: the paper's knot insertion operator applied to the cubic Bernstein polynomials.
fn spline(t: f64, r: usize) -> [f64; 4] {
    const M: [[f64; 4]; 4] = [
        [1.0, 0.0, 0.0, 0.0],
        [4.0, 4.0, 2.0, 1.0],
        [1.0, 2.0, 4.0, 4.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let b = bernstein(3, t, r);
    let mut out = [0.0; 4];
    for (row, o) in M.iter().zip(&mut out) {
        *o = row.iter().zip(&b).map(|(m, b)| m * b).sum::<f64>() / 6.0;
    }
    out
}

/// The cosine of the angle between two spokes of a corner of `n` faces.
fn cosine(n: usize) -> f64 {
    (2.0 * std::f64::consts::PI / n as f64).cos()
}

/// The Bézier points of the x coordinate of `psi_n`, degree 4 in u by 3 in v (the paper's
/// Section 3.2, with the two free values of its Appendix A); the y coordinate is the same with u
/// and v changed over, and `psi_4` is the identity.
fn psi_points(n: usize) -> [[f64; 4]; 5] {
    let c = cosine(n);
    [
        [0.0, 0.0, 0.0, 0.0],
        [
            0.25,
            (5.0 * c * c * c - 3.0 * c * c - 15.0 * c + 18.0)
                / (12.0 * (c - 2.0) * (2.0 * c - 3.0)),
            (c * c + 2.0 * c - 6.0) / (12.0 * (c - 2.0)),
            0.25,
        ],
        [
            0.5,
            (c + 3.0) / 6.0,
            (c * c + 3.0 * c - 9.0) / (9.0 * (c - 2.0)),
            0.5,
        ],
        [0.75, (c + 9.0) / 12.0, (c + 9.0) / 12.0, 0.75],
        [1.0, 1.0, 1.0, 1.0],
    ]
}

/// The derivative of orders (ru, rv) of `psi_n`'s x coordinate at (u, v).
fn psi(points: &[[f64; 4]; 5], u: f64, v: f64, ru: usize, rv: usize) -> f64 {
    let (bu, bv) = (bernstein(4, u, ru), bernstein(3, v, rv));
    points
        .iter()
        .zip(&bu)
        .map(|(row, wu)| wu * row.iter().zip(&bv).map(|(p, wv)| p * wv).sum::<f64>())
        .sum()
}

// ---- The conditions ------------------------------------------------------------------------

/// A row of the conditions: a sum over the patches' points equals a sum over the two-ring.
#[derive(Default)]
struct Row {
    patches: Vec<(usize, f64)>,
    ring: Vec<(usize, f64)>,
}

/// Patch `i`'s derivative of orders (ru, rv) at (u, v), as a weight on each of its points.
fn patch_derivative(i: usize, u: f64, v: f64, ru: usize, rv: usize) -> Vec<(usize, f64)> {
    let (bu, bv) = (bernstein(DEGREE, u, ru), bernstein(DEGREE, v, rv));
    let mut out = Vec::with_capacity(POINTS);
    for (b, wv) in bv.iter().enumerate() {
        for (a, wu) in bu.iter().enumerate() {
            let w = wu * wv;
            if w != 0.0 {
                out.push((i * POINTS + a + (DEGREE + 1) * b, w));
            }
        }
    }
    out
}

/// The bicubic ring patch `H_i` (the paper's Figure 3 and its 4 by 4 grid): which two-ring point
/// each grid point is, row by row along u; the one corner no derivative on the outer edges
/// reads is none.
fn ring_grid(n: usize, i: usize) -> Result<[[Option<usize>; 4]; 4]> {
    let i = i64::try_from(i).map_err(|_| bad("too many faces"))?;
    let at = |s: i64, jk: (u8, u8)| ring_index(n, s, jk).map(Some);
    Ok([
        [
            None,
            at(i + 2, (1, 0))?,
            at(i + 1, (1, 1))?,
            at(i + 1, (2, 1))?,
        ],
        [
            at(i - 1, (1, 0))?,
            Some(0),
            at(i + 1, (1, 0))?,
            at(i + 1, (2, 0))?,
        ],
        [
            at(i - 1, (1, 1))?,
            at(i, (1, 0))?,
            at(i, (1, 1))?,
            at(i, (1, 2))?,
        ],
        [
            at(i - 1, (1, 2))?,
            at(i, (2, 0))?,
            at(i, (2, 1))?,
            at(i, (2, 2))?,
        ],
    ])
}

/// `H_i`'s derivative of orders (rx, ry) at (x, y), as a weight on each two-ring point, times
/// `scale`, added to `into`.
fn ring_derivative(
    grid: &[[Option<usize>; 4]; 4],
    (x, y): (f64, f64),
    (rx, ry): (usize, usize),
    scale: f64,
    into: &mut Vec<(usize, f64)>,
) -> Result<()> {
    let (sx, sy) = (spline(x, rx), spline(y, ry));
    for (row, wx) in grid.iter().zip(sx) {
        for (point, wy) in row.iter().zip(sy) {
            let w = wx * wy * scale;
            if w.abs() < 1e-15 {
                continue;
            }
            // The outer edges read no weight of the grid's undefined corner.
            let k = point.ok_or_else(|| fail("an outer edge read the ring's undefined corner"))?;
            into.push((k, w));
        }
    }
    Ok(())
}

/// Where along an edge each condition is held: [`PLACES`] of them, spread toward the ends.
fn places() -> Vec<f64> {
    (0..PLACES)
        .map(|k| 0.5 - 0.5 * (std::f64::consts::PI * (k as f64 + 0.5) / PLACES as f64).cos())
        .collect()
}

/// The paper's external conditions (equations 5 to 7): patch `i` meets the ring patch `H_i`
/// after `psi_n` in G2 on its outer edges, u = 1 and v = 1.
fn external(n: usize, i: usize, rows: &mut Vec<Row>) -> Result<()> {
    let grid = ring_grid(n, i)?;
    let px = psi_points(n);
    for t in places() {
        // Along u = 1 the derivatives across are in u; the y coordinate of psi is its x
        // coordinate with u and v changed over, so its u derivative at (1, t) is the x
        // coordinate's v derivative at (t, 1). Along v = 1, the same with u and v changed over.
        let first = (psi(&px, 1.0, t, 1, 0), psi(&px, t, 1.0, 0, 1));
        let second = (psi(&px, 1.0, t, 2, 0), psi(&px, t, 1.0, 0, 2));
        for along_v in [true, false] {
            let (u, v) = if along_v { (1.0, t) } else { (t, 1.0) };
            let ((p, q), (pp, qq)) = if along_v {
                (first, second)
            } else {
                ((first.1, first.0), (second.1, second.0))
            };
            for r in 0..=2_usize {
                let (ru, rv) = if along_v { (r, 0) } else { (0, r) };
                let mut row = Row {
                    patches: patch_derivative(i, u, v, ru, rv),
                    ring: Vec::new(),
                };
                match r {
                    0 => ring_derivative(&grid, (u, v), (0, 0), 1.0, &mut row.ring)?,
                    1 => {
                        ring_derivative(&grid, (u, v), (1, 0), p, &mut row.ring)?;
                        ring_derivative(&grid, (u, v), (0, 1), q, &mut row.ring)?;
                    }
                    _ => {
                        ring_derivative(&grid, (u, v), (1, 0), pp, &mut row.ring)?;
                        ring_derivative(&grid, (u, v), (0, 1), qq, &mut row.ring)?;
                        ring_derivative(&grid, (u, v), (2, 0), p * p, &mut row.ring)?;
                        ring_derivative(&grid, (u, v), (1, 1), 2.0 * p * q, &mut row.ring)?;
                        ring_derivative(&grid, (u, v), (0, 2), q * q, &mut row.ring)?;
                    }
                }
                rows.push(row);
            }
        }
    }
    Ok(())
}

/// The corner itself, where the patches all meet, at five faces and more: the Catmull and Clark
/// limit point of the two-ring, each spoke's end weighed 4 and each face's far corner 1 against
/// the corner's `n (n + 5) - 5 n` (Karčiauskas and Peters, *Minimal bi-6 G2 completion of bicubic
/// spline surfaces*, 2016, step 1 of their algorithm). Left to the energy, the corner sinks below
/// the faces round it (their Fig. 7: "a noticeable dip"). On the shape bench (a ball fitted to a
/// sphere, `crates/scan/src/cage_fit.rs`) it takes the five-faced corners' faces from 14.4 % off
/// the sphere's curvature to 6.0 %, as true as the regular faces there (5.4 %). At three faces it
/// does not help this construction (16.5 % free, 17.5 % set; their 1/96 more on the far corners,
/// tuned for their bi-6 patches, 30 %): the corner is left to the energy there. At four faces it
/// would be the bicubic B-spline's own corner point.
fn corner_point(n: usize) -> Result<Row> {
    let faces = f64::from(u32::try_from(n).map_err(|_| bad("too many faces"))?);
    let whole = faces * (faces + 5.0);
    let (spoke, far) = (4.0 / whole, 1.0 / whole);
    let mut row = Row {
        patches: vec![(0, 1.0)],
        ring: vec![(0, 1.0 - faces * (spoke + far))],
    };
    for i in 0..n {
        let sector = i64::try_from(i).map_err(|_| bad("too many faces"))?;
        row.ring.push((ring_index(n, sector, (1, 0))?, spoke));
        row.ring.push((ring_index(n, sector, (1, 1))?, far));
    }
    Ok(row)
}

/// The paper's internal conditions (equations 8 to 10) between `P_i(0, t)` and `P_{i+1}(t, 0)`.
fn internal(n: usize, i: usize, rows: &mut Vec<Row>) {
    let c = cosine(n);
    let j = (i + 1) % n;
    let row_of = |terms: Vec<(Vec<(usize, f64)>, f64)>| {
        let mut row = Row::default();
        for (functional, scale) in terms {
            row.patches
                .extend(functional.into_iter().map(|(k, w)| (k, w * scale)));
        }
        row
    };
    for t in places() {
        let mine = |ru, rv| patch_derivative(i, 0.0, t, ru, rv);
        let next = |ru, rv| patch_derivative(j, t, 0.0, ru, rv);
        let s = 1.0 - t;
        // (8) The patches share the edge.
        rows.push(row_of(vec![(mine(0, 0), 1.0), (next(0, 0), -1.0)]));
        // (9) c (1 - t) (d/du P_{i+1} + d/dv P_i) = d/dv P_{i+1} + d/du P_i.
        rows.push(row_of(vec![
            (next(1, 0), c * s),
            (mine(0, 1), c * s),
            (next(0, 1), -1.0),
            (mine(1, 0), -1.0),
        ]));
        // (10) 2 c^2 (1 - t) (d/dv P_{i+1} - d/du P_i)
        //      + 2 c (1 - t) (1 + c (1 - t)) (d2/dudv P_{i+1} - d2/dudv P_i)
        //      = (1 + c (1 - t)) (d2/dv2 P_{i+1} - d2/du2 P_i).
        let k = 1.0 + c * s;
        rows.push(row_of(vec![
            (next(0, 1), 2.0 * c * c * s),
            (mine(1, 0), -2.0 * c * c * s),
            (next(1, 1), 2.0 * c * s * k),
            (mine(1, 1), -2.0 * c * s * k),
            (next(0, 2), -k),
            (mine(2, 0), k),
        ]));
    }
}

// ---- The energy ----------------------------------------------------------------------------

/// The Gram matrix of the degree-7 Bernstein polynomials' `r`-th derivatives over [0, 1], by an
/// 8-point Gauss rule (exact to degree 15).
fn gram(r: usize) -> [[f64; DEGREE + 1]; DEGREE + 1] {
    const NODES: [(f64, f64); 8] = [
        (-0.960_289_856_497_536_3, 0.101_228_536_290_376_26),
        (-0.796_666_477_413_626_7, 0.222_381_034_453_374_47),
        (-0.525_532_409_916_329, 0.313_706_645_877_887_3),
        (-0.183_434_642_495_649_8, 0.362_683_783_378_362),
        (0.183_434_642_495_649_8, 0.362_683_783_378_362),
        (0.525_532_409_916_329, 0.313_706_645_877_887_3),
        (0.796_666_477_413_626_7, 0.222_381_034_453_374_47),
        (0.960_289_856_497_536_3, 0.101_228_536_290_376_26),
    ];
    let mut g = [[0.0; DEGREE + 1]; DEGREE + 1];
    for (x, w) in NODES {
        let b = bernstein(DEGREE, 0.5 * (x + 1.0), r);
        for (row, ba) in g.iter_mut().zip(&b) {
            for (cell, bc) in row.iter_mut().zip(&b) {
                *cell += 0.5 * w * ba * bc;
            }
        }
    }
    g
}

/// One patch's bicubic energy as a matrix over its points, row after row: the Gram matrix of the
/// fourth derivatives along u with that of the values along v, and the other way about.
fn energy() -> Vec<f64> {
    let (g0, g4) = (gram(0), gram(4));
    let mut e = Vec::with_capacity(POINTS * POINTS);
    // The row of point (a, b), then the column of point (c, d): points run a first.
    for (g0b, g4b) in g0.iter().zip(&g4) {
        for (g0a, g4a) in g0.iter().zip(&g4) {
            for (g0bd, g4bd) in g0b.iter().zip(g4b) {
                for (g0ac, g4ac) in g0a.iter().zip(g4a) {
                    e.push(g4ac * g0bd + g0ac * g4bd);
                }
            }
        }
    }
    e
}

/// The energy matrix times `x`, patch by patch.
fn energy_times(one: &[f64], x: &[f64]) -> Vec<f64> {
    let mut out = Vec::with_capacity(x.len());
    for patch in x.chunks_exact(POINTS) {
        for row in one.chunks_exact(POINTS) {
            out.push(row.iter().zip(patch).map(|(e, x)| e * x).sum());
        }
    }
    out
}

// ---- The solve -----------------------------------------------------------------------------

/// The conditions' rows as the columns of one matrix, factored by Householder reflections with
/// column pivoting: the rows that say something new first, each made upper triangular against
/// those before it.
struct Factored {
    /// The length of a row: the patches' points.
    unknowns: usize,
    /// The rows, each of `unknowns` numbers, as the factorisation left them: above its own
    /// place a row holds its part along the rows taken before it.
    rows: Vec<f64>,
    /// A reflection for each row taken: its vector (from the row's own place down) and two over
    /// that vector's squared length.
    reflections: Vec<(Vec<f64>, f64)>,
    /// Each taken row's own part, once the rows before it are reflected away.
    own: Vec<f64>,
    /// Which condition each row was to begin with.
    order: Vec<usize>,
    /// The shortest row taken and the longest left out, each against the longest of all.
    separation: (f64, f64),
}

impl Factored {
    /// How many rows say something the others do not.
    fn rank(&self) -> usize {
        self.own.len()
    }

    /// The reflections applied to `x`, the last first: a vector of the factored frame carried
    /// back to the patches' points.
    fn back(&self, x: &mut [f64]) -> Result<()> {
        for (k, (v, beta)) in self.reflections.iter().enumerate().rev() {
            let tail = within(x.get_mut(k..))?;
            let s = beta * tail.iter().zip(v).map(|(a, b)| a * b).sum::<f64>();
            for (a, b) in tail.iter_mut().zip(v) {
                *a -= s * b;
            }
        }
        Ok(())
    }

    /// The shortest set of patch points that meets the taken conditions with the right-hand
    /// sides `rhs` (one for each condition, in the conditions' first order).
    fn particular(&self, rhs: &[f64]) -> Result<Vec<f64>> {
        let mut y = vec![0.0; self.unknowns];
        for (j, own) in self.own.iter().enumerate() {
            let row = within(self.rows.get(j * self.unknowns..j * self.unknowns + j))?;
            let before = within(y.get(..j))?;
            let along: f64 = row.iter().zip(before).map(|(r, y)| r * y).sum();
            let b = within(self.order.get(j).and_then(|&o| rhs.get(o)))?;
            *within(y.get_mut(j))? = (b - along) / own;
        }
        self.back(&mut y)?;
        Ok(y)
    }

    /// A unit vector of the space the conditions leave free: the `f`-th.
    fn free(&self, f: usize) -> Result<Vec<f64>> {
        let mut e = vec![0.0; self.unknowns];
        *within(e.get_mut(self.rank() + f))? = 1.0;
        self.back(&mut e)?;
        Ok(e)
    }
}

/// `rows` (each of `unknowns` numbers) factored: see [`Factored`].
fn factor(mut rows: Vec<f64>, unknowns: usize) -> Result<Factored> {
    let count = rows.len() / unknowns.max(1);
    let mut order: Vec<usize> = (0..count).collect();
    let (mut reflections, mut own) = (Vec::new(), Vec::new());
    let mut longest = 0.0_f64;
    let mut separation = (1.0_f64, 0.0_f64);
    for k in 0..unknowns.min(count) {
        // The remaining row that is longest once the rows taken are reflected away.
        let mut best = (k, -1.0_f64);
        for (j, row) in rows.chunks_exact(unknowns).enumerate().skip(k) {
            let length: f64 = within(row.get(k..))?.iter().map(|x| x * x).sum();
            if length > best.1 {
                best = (j, length);
            }
        }
        let length = best.1.sqrt();
        if k == 0 {
            longest = length;
        }
        // Written to accept: a length that is not a number stops here too.
        let says_something_new = length > longest * REPEATS;
        if !says_something_new {
            separation.1 = length / longest;
            break;
        }
        separation.0 = length / longest;
        if best.0 != k {
            let (head, tail) = rows.split_at_mut(best.0 * unknowns);
            let a = within(head.get_mut(k * unknowns..(k + 1) * unknowns))?;
            let b = within(tail.get_mut(..unknowns))?;
            a.swap_with_slice(b);
            order.swap(k, best.0);
        }
        let row = within(rows.get_mut(k * unknowns..(k + 1) * unknowns))?;
        let mut v = within(row.get(k..))?.to_vec();
        let lead = within(v.first_mut())?;
        let alpha = if *lead > 0.0 { -length } else { length };
        *lead -= alpha;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        let has_length = vv > 0.0;
        if !has_length {
            return Err(fail("a condition's reflection has no length"));
        }
        let beta = 2.0 / vv;
        for later in rows.chunks_exact_mut(unknowns).skip(k + 1) {
            let tail = within(later.get_mut(k..))?;
            let s = beta * tail.iter().zip(&v).map(|(a, b)| a * b).sum::<f64>();
            for (a, b) in tail.iter_mut().zip(&v) {
                *a -= s * b;
            }
        }
        reflections.push((v, beta));
        own.push(alpha);
    }
    Ok(Factored {
        unknowns,
        rows,
        reflections,
        own,
        order,
        separation,
    })
}

/// The lower triangle `l` of a symmetric matrix made its Cholesky factor, in place.
fn cholesky(l: &mut [Vec<f64>]) -> Result<()> {
    let largest = l
        .iter()
        .enumerate()
        .filter_map(|(i, row)| row.get(i))
        .fold(0.0_f64, |m, x| m.max(x.abs()));
    for j in 0..l.len() {
        let (row, below) = within(within(l.get_mut(j..))?.split_first_mut())?;
        let lead = within(row.get(..j))?.to_vec();
        let d = within(row.get(j).copied())? - lead.iter().map(|x| x * x).sum::<f64>();
        // Written to accept: a pivot that is not a number refuses too.
        let fixed = d > largest * UNFIXED;
        if !fixed {
            return Err(fail("the energy does not fix the patches"));
        }
        let d = d.sqrt();
        *within(row.get_mut(j))? = d;
        for other in below {
            let s: f64 = within(other.get(..j))?
                .iter()
                .zip(&lead)
                .map(|(a, b)| a * b)
                .sum();
            let x = within(other.get_mut(j))?;
            *x = (*x - s) / d;
        }
    }
    Ok(())
}

/// The answer `z` of `L L^T z = rhs` for a Cholesky factor `l`.
fn cholesky_solve(l: &[Vec<f64>], rhs: &[f64]) -> Result<Vec<f64>> {
    let mut y: Vec<f64> = Vec::with_capacity(rhs.len());
    for (i, (row, b)) in l.iter().zip(rhs).enumerate() {
        let s: f64 = within(row.get(..i))?
            .iter()
            .zip(&y)
            .map(|(a, b)| a * b)
            .sum();
        y.push((b - s) / within(row.get(i).copied())?);
    }
    let mut z = vec![0.0; y.len()];
    for i in (0..y.len()).rev() {
        let mut s = 0.0;
        for (row, zk) in l.iter().zip(&z).skip(i + 1) {
            s += within(row.get(i).copied())? * zk;
        }
        let d = within(l.get(i).and_then(|row| row.get(i)).copied())?;
        *within(z.get_mut(i))? = (within(y.get(i).copied())? - s) / d;
    }
    Ok(z)
}

/// The blends for `n` faces: the least bicubic energy under the G2 conditions, for each two-ring
/// point in turn set to one.
fn solve(n: usize) -> Result<CornerBasis> {
    let mut conditions = Vec::new();
    for i in 0..n {
        external(n, i, &mut conditions)?;
        internal(n, i, &mut conditions);
    }
    if n >= 5 {
        conditions.push(corner_point(n)?);
    }
    let (unknowns, columns) = (n * POINTS, 1 + 6 * n);
    // The conditions as dense rows over the patches' points, each scaled to unit length so that
    // they weigh alike, and their right-hand sides over the two-ring.
    let mut rows = vec![0.0; conditions.len() * unknowns];
    let mut sides = vec![0.0; conditions.len() * columns];
    for ((c, row), side) in conditions
        .iter()
        .zip(rows.chunks_exact_mut(unknowns))
        .zip(sides.chunks_exact_mut(columns))
    {
        for &(k, w) in &c.patches {
            *within(row.get_mut(k))? += w;
        }
        let scale = row.iter().map(|w| w * w).sum::<f64>().sqrt().max(1e-300);
        for w in row.iter_mut() {
            *w /= scale;
        }
        for &(k, w) in &c.ring {
            *within(side.get_mut(k))? += w / scale;
        }
    }
    let kept = rows.clone();
    let factored = factor(rows, unknowns)?;
    let rank = factored.rank();
    // The space the conditions leave free, the energy on it, and that energy factored.
    let one = energy();
    let free: Vec<Vec<f64>> = (0..unknowns - rank)
        .map(|f| factored.free(f))
        .collect::<Result<_>>()?;
    let energy_free: Vec<Vec<f64>> = free.iter().map(|x| energy_times(&one, x)).collect();
    let mut reduced: Vec<Vec<f64>> = free
        .iter()
        .map(|f| {
            energy_free
                .iter()
                .map(|e| f.iter().zip(e).map(|(a, b)| a * b).sum())
                .collect()
        })
        .collect();
    cholesky(&mut reduced)?;
    // For each two-ring point set to one: a set of patch points that meets the conditions, then
    // the one of least energy among all that do.
    let mut weights = vec![0.0; unknowns * columns];
    for col in 0..columns {
        let rhs: Vec<f64> = sides.iter().skip(col).step_by(columns).copied().collect();
        let p0 = factored.particular(&rhs)?;
        let pull: Vec<f64> = energy_free
            .iter()
            .map(|e| -e.iter().zip(&p0).map(|(a, b)| a * b).sum::<f64>())
            .collect();
        let z = cholesky_solve(&reduced, &pull)?;
        let mut answer = p0;
        for (f, zf) in free.iter().zip(&z) {
            for (a, x) in answer.iter_mut().zip(f) {
                *a += x * zf;
            }
        }
        // Every condition holds of the answer: a wrong condition (or one written down wrongly)
        // would leave some unmet.
        for (row, b) in kept.chunks_exact(unknowns).zip(&rhs) {
            let lhs: f64 = row.iter().zip(&answer).map(|(a, x)| a * x).sum();
            // Written to accept: a sum that is not a number refuses too.
            let held = (lhs - b).abs() <= HELD;
            if !held {
                return Err(fail("a G2 condition is not held by the blends"));
            }
        }
        for (k, value) in answer.iter().enumerate() {
            *within(weights.get_mut(k * columns + col))? = *value;
        }
    }
    Ok(CornerBasis {
        faces: n,
        conditions: rank,
        separation: factored.separation,
        weights,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The most faces the quick tests make a basis for: the work grows with the cube of the
    /// faces, and an unoptimised build takes seconds for each of the larger ones.
    const QUICK: usize = 8;

    /// The basis for `n` faces, made once for all the tests.
    fn basis(n: usize) -> std::sync::Arc<CornerBasis> {
        static MADE: std::sync::Mutex<Vec<(usize, std::sync::Arc<CornerBasis>)>> =
            std::sync::Mutex::new(Vec::new());
        let mut made = MADE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, b)) = made.iter().find(|(k, _)| *k == n) {
            return b.clone();
        }
        let b = std::sync::Arc::new(CornerBasis::for_faces(n).unwrap());
        made.push((n, b.clone()));
        b
    }

    type V = [f64; 3];

    fn sub(a: V, b: V) -> V {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn dot(a: V, b: V) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn cross(a: V, b: V) -> V {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    }

    fn length(a: V) -> f64 {
        dot(a, a).sqrt()
    }

    fn v(p: Point3) -> V {
        [p.x, p.y, p.z]
    }

    /// A patch's point or derivative at (u, v), from its Bézier points.
    fn evaluate(points: &[Point3], u: f64, w: f64, ru: usize, rv: usize) -> V {
        let (bu, bv) = (bernstein(DEGREE, u, ru), bernstein(DEGREE, w, rv));
        let mut s = [0.0; 3];
        for b in 0..=DEGREE {
            for a in 0..=DEGREE {
                let p = v(points[a + (DEGREE + 1) * b]);
                for k in 0..3 {
                    s[k] += p[k] * bu[a] * bv[b];
                }
            }
        }
        s
    }

    /// The bicubic ring patch's point or derivative at (x, y), the undefined corner given
    /// `corner`.
    fn ring_evaluate(
        n: usize,
        i: usize,
        ring: &[Point3],
        corner: V,
        (x, y): (f64, f64),
        (rx, ry): (usize, usize),
    ) -> V {
        let grid = ring_grid(n, i).unwrap();
        let (sx, sy) = (spline(x, rx), spline(y, ry));
        let mut s = [0.0; 3];
        for r in 0..4 {
            for c in 0..4 {
                let p = grid[r][c].map_or(corner, |k| v(ring[k]));
                for k in 0..3 {
                    s[k] += p[k] * sx[r] * sy[c];
                }
            }
        }
        s
    }

    /// The surface's second fundamental form as a tensor in space, whatever the parameters, and
    /// its unit normal: what two surfaces share where they meet in G2.
    fn curvature_tensor(f: &dyn Fn(usize, usize) -> V) -> ([[f64; 3]; 3], V) {
        let (su, sv, suu, suv, svv) = (f(1, 0), f(0, 1), f(2, 0), f(1, 1), f(0, 2));
        let normal = cross(su, sv);
        let size = length(normal);
        let normal = [normal[0] / size, normal[1] / size, normal[2] / size];
        let (e, ff, g) = (dot(su, su), dot(su, sv), dot(sv, sv));
        let (l, m, nn) = (dot(suu, normal), dot(suv, normal), dot(svv, normal));
        let det = e * g - ff * ff;
        let inverse = [[g / det, -ff / det], [-ff / det, e / det]];
        let ii = [[l, m], [m, nn]];
        let mut middle = [[0.0; 2]; 2];
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    for d in 0..2 {
                        middle[a][d] += inverse[a][b] * ii[b][c] * inverse[c][d];
                    }
                }
            }
        }
        let frame = [su, sv];
        let mut out = [[0.0; 3]; 3];
        for a in 0..2 {
            for d in 0..2 {
                for r in 0..3 {
                    for c in 0..3 {
                        out[r][c] += middle[a][d] * frame[a][r] * frame[d][c];
                    }
                }
            }
        }
        (out, normal)
    }

    /// How far apart two tensors are, and two normals.
    fn apart(a: &([[f64; 3]; 3], V), b: &([[f64; 3]; 3], V)) -> (f64, f64) {
        let mut tensor = 0.0_f64;
        for r in 0..3 {
            for c in 0..3 {
                tensor = tensor.max((a.0[r][c] - b.0[r][c]).abs());
            }
        }
        (tensor, length(sub(a.1, b.1)))
    }

    /// A two-ring on a surface z = f(x, y) round a corner of n faces, the spokes spread evenly.
    fn ring_on(n: usize, f: impl Fn(f64, f64) -> f64) -> Vec<Point3> {
        let mut ring = vec![Point3::new(0.0, 0.0, f(0.0, 0.0))];
        for i in 0..n {
            let angle = |k: f64| 2.0 * std::f64::consts::PI * (i as f64 + k) / n as f64;
            for (j, k) in SECTOR {
                // Out along the spoke j steps, across toward the next spoke k steps.
                let (r, turn) = match (j, k) {
                    (1, 0) => (1.0, 0.0),
                    (2, 0) => (2.0, 0.0),
                    (1, 1) => (1.4, 0.5),
                    (2, 1) => (2.2, 0.3),
                    (1, 2) => (2.2, 0.7),
                    _ => (2.8, 0.5),
                };
                let (x, y) = (r * angle(turn).cos(), r * angle(turn).sin());
                ring.push(Point3::new(x, y, f(x, y)));
            }
        }
        ring
    }

    /// At four faces the patches are the bicubic B-spline's own: the least energy is nothing,
    /// for a bicubic.
    #[test]
    fn four_faces_give_back_the_bicubic_b_spline() {
        let basis = basis(4);
        let ring = ring_on(4, |x, y| 0.3 * (x * 1.3).sin() + 0.2 * x * y);
        let patches = basis.patches(&ring).unwrap();
        for i in 0..4 {
            // The undefined corner of H_i, at four faces, is sector i + 2's far corner.
            let corner = v(ring[ring_index(4, i as i64 + 2, (1, 1)).unwrap()]);
            for (u, w) in [(0.1, 0.2), (0.5, 0.5), (0.9, 0.3), (0.3, 0.95)] {
                let p = evaluate(&patches[i * POINTS..(i + 1) * POINTS], u, w, 0, 0);
                let h = ring_evaluate(4, i, &ring, corner, (u, w), (0, 0));
                assert!(
                    length(sub(p, h)) < 1e-9,
                    "patch {i} at ({u}, {w}): {p:?} against {h:?}"
                );
            }
        }
    }

    /// At 3, 5, 6 and 8 faces the patches meet one another and the bicubic ring in curvature:
    /// the curvature tensor and the normal the same from either side at every place tried along
    /// each edge.
    #[test]
    fn the_patches_meet_each_other_and_the_ring_in_curvature() {
        for n in [3, 5, 6, 8] {
            let basis = basis(n);
            let ring = ring_on(n, |x, y| 0.3 * (x * 1.3).sin() + 0.2 * x * y - 0.1 * y * y);
            let patches = basis.patches(&ring).unwrap();
            let patch = |i: usize| &patches[(i % n) * POINTS..(i % n + 1) * POINTS];
            let mut worst = (0.0_f64, 0.0_f64);
            for i in 0..n {
                for t in [0.05, 0.3, 0.6, 0.95] {
                    // Between patch i and the next.
                    let a = curvature_tensor(&|ru, rv| evaluate(patch(i), 0.0, t, ru, rv));
                    let b = curvature_tensor(&|ru, rv| evaluate(patch(i + 1), t, 0.0, ru, rv));
                    let (dt, dn) = apart(&a, &b);
                    worst = (worst.0.max(dt), worst.1.max(dn));
                    // With the ring, on u = 1 and v = 1.
                    for (u, w) in [(1.0, t), (t, 1.0)] {
                        let a = curvature_tensor(&|ru, rv| evaluate(patch(i), u, w, ru, rv));
                        let b = curvature_tensor(&|ru, rv| {
                            ring_evaluate(n, i, &ring, [0.0; 3], (u, w), (ru, rv))
                        });
                        let (dt, dn) = apart(&a, &b);
                        worst = (worst.0.max(dt), worst.1.max(dn));
                    }
                }
            }
            assert!(
                worst.0 < 1e-7 && worst.1 < 1e-9,
                "{n} faces: curvature apart by {}, normals by {}",
                worst.0,
                worst.1
            );
        }
    }

    /// Each point of a patch is a blend whose weights sum to one, so a two-ring on a plane gives
    /// patches on it; and a place on a patch as a blend is the patch's point there.
    #[test]
    fn the_weights_sum_to_one_and_a_plane_stays_a_plane() {
        for n in [3, 5, 7] {
            let basis = basis(n);
            for k in 0..n * POINTS {
                let sum: f64 = basis.blend(k / POINTS, k % POINTS).unwrap().iter().sum();
                assert!((sum - 1.0).abs() < 1e-9, "{n} faces, point {k}: {sum}");
            }
            let ring = ring_on(n, |x, y| 2.0 + 0.5 * x - 0.25 * y);
            let patches = basis.patches(&ring).unwrap();
            assert!(patches
                .iter()
                .all(|p| (p.z - (2.0 + 0.5 * p.x - 0.25 * p.y)).abs() < 1e-9));
            let blend = basis.place_blend(1, 0.3, 0.6).unwrap();
            let mut blended = [0.0; 3];
            for (w, p) in blend.iter().zip(&ring) {
                for (b, x) in blended.iter_mut().zip(v(*p)) {
                    *b += w * x;
                }
            }
            let there = evaluate(&patches[POINTS..2 * POINTS], 0.3, 0.6, 0, 0);
            assert!(length(sub(blended, there)) < 1e-9);
        }
    }

    /// Of the paper's 55 conditions a face, six follow from the rest when all the faces are taken
    /// together (at 5 faces and at 7 or more), and the factorisation tells them apart by many
    /// orders: the shortest condition taken is far longer than the longest left out. From five
    /// faces the corner point adds one more, which none of the others implies.
    #[test]
    fn the_repeated_conditions_are_told_from_the_rest() {
        for n in [5, 7, 8] {
            let basis = basis(n);
            assert_eq!(basis.conditions(), 55 * n - 6 + 1, "{n} faces");
        }
        for n in MIN_FACES..=QUICK {
            let (taken, left) = basis(n).separation();
            assert!(
                taken > 1e-6 && left < 1e-11,
                "{n} faces: {taken:e} taken, {left:e} left out"
            );
        }
    }

    /// The weights are nearly all positive: the least is a few thousandths below nothing, as
    /// the paper says of its own, so a patch leaves its two-ring's hull by a hair at most.
    #[test]
    fn the_least_weight_is_a_few_thousandths_below_nothing() {
        for n in MIN_FACES..=QUICK {
            let basis = basis(n);
            let mut least = f64::MAX;
            for k in 0..n * POINTS {
                for w in basis.blend(k / POINTS, k % POINTS).unwrap() {
                    least = least.min(*w);
                }
            }
            assert!(least > -1e-2, "{n} faces: {least}");
        }
    }

    /// The corners of 9 to 12 faces, which take long in an unoptimised build: the same checks as
    /// the quick tests make of the smaller ones. Run with `-- --ignored`.
    #[test]
    #[ignore = "seconds for each size in an unoptimised build"]
    fn the_larger_corners_hold_too() {
        for n in QUICK + 1..=MAX_FACES {
            let basis = basis(n);
            assert_eq!(basis.conditions(), 55 * n - 6, "{n} faces");
            let (taken, left) = basis.separation();
            assert!(
                taken > 1e-6 && left < 1e-11,
                "{n} faces: {taken:e} taken, {left:e} left out"
            );
            let mut least = f64::MAX;
            for k in 0..n * POINTS {
                let blend = basis.blend(k / POINTS, k % POINTS).unwrap();
                let sum: f64 = blend.iter().sum();
                assert!((sum - 1.0).abs() < 1e-9, "{n} faces, point {k}: {sum}");
                least = blend.iter().fold(least, |m, w| m.min(*w));
            }
            assert!(least > -1e-2, "{n} faces: {least}");
        }
    }

    #[test]
    fn a_corner_of_too_few_or_too_many_faces_has_no_basis() {
        assert!(CornerBasis::for_faces(2).is_err() && CornerBasis::for_faces(13).is_err());
        assert!(ring_index(5, 0, (3, 3)).is_err() && ring_index(0, 0, (1, 0)).is_err());
        let basis = basis(3);
        assert!(basis.blend(3, 0).is_err() && basis.blend(0, POINTS).is_err());
        assert!(basis.patches(&[Point3::new(0.0, 0.0, 0.0)]).is_err());
    }
}
