//! The smooth surface of a quad cage, built the way a Freeform cage editor builds it: bicubic
//! faces that pass through the cage's Catmull-Clark limit points, laid out as rectangles of quads.
//!
//! Upstream: none (solidcore-native). The construction at irregular corners is J. Fan and
//! J. Peters, *Smooth bi-3 spline surfaces with fewest knots*, Computer-Aided Design 43(2), 2011
//! (Sections 2.1-2.8), with the changes below, which make the surface match the one `QuickSurface`'s
//! Freeform editor exports for the same cage (each measured on its exports of cages with known
//! points, to 1e-6 mm; the measurements and their record are outside this repository). The layout
//! of the faces follows the motorcycle graph of D. Eppstein, M. T. Goodrich, E. Kim and
//! R. Tamstorf, *Motorcycle graphs: canonical quad mesh partitioning*, SGP 2008, traced one line at
//! a time instead of all together (below).
//!
//! | Rust                        | Upstream |
//! |-----------------------------|----------|
//! | [`CageSurface::of`]         | none     |
//! | [`CageFace`]                | none     |
//! | [`CageFace::points`]        | none     |
//!
//! **What is built.** Every quad gets a bicubic patch with two double knots each way (knots
//! 0, 0, 0, 0, 1/3, 1/3, 2/3, 2/3, 1, 1, 1, 1): its 8 x 8 coefficients sit at the Greville places
//! 0, 1, 2, 4, 5, 7, 8, 9 (in ninths). A quad whose four corners have four quads each is the
//! uniform bicubic B-spline of the sixteen cage points round it. A quad with an irregular corner
//! is built in the paper's five stages:
//! 1. the Bézier patch of the paper's eq. 5-7, its corner the Catmull-Clark limit point, the knots
//!    inserted;
//! 2. at an irregular corner the two tangent points from the Catmull-Clark tangent masks scaled by
//!    `sigma` (0.53 for three quads, `1/(4 lambda)` otherwise), and at every corner the twist
//!    `(6 (b10 + b01) - 4 b00 + q11) / 9`;
//! 3. the boundary: `b20` from the G1 condition at an irregular end, then
//!    `b40 = 4/3 b20 + 2/3 b70 - 1/3 b80 - 2/3 b10` from both ends of every edge that touches an
//!    irregular corner (the paper keeps the regular end's `b20` and `b40`; the measured surface
//!    rebuilds `b40` there);
//! 4. the first inner row along those edges from the G1 condition: `alpha(u) d/du b(u, 0)`
//!    projected onto the patch's splines (exact L2 projection; `alpha` is linear from
//!    `2 cos(2 pi / n0)` to `-2 cos(2 pi / n1)`, each term absent at a regular end), half to each
//!    side, plus half the difference of the paper's preliminary rows (its step 4a);
//! 5. the inner points by the paper's averaging, at every corner of the quad (the paper skips
//!    regular corners; the measured surface does not).
//!
//! **Faces.** The quads are then grouped into rectangular faces, and inside a face the quads are
//! joined with single knots: across every joint inside a face, one direction after the other (the
//! two passes commute), the first rows become `b_1 = b_0 + (b_2 - b'_2) / 4` and `b'_1 = 2 b_0 -
//! b_1` at every place along the joint, which is what removing the joint's triple knot down to a
//! single one needs (C2 across it). Across a face border both sides keep their rows (C1 there). So
//! the layout is part of the surface. It is:
//! - lines from every irregular vertex along each of its edges, straight on through vertices of
//!   four quads. A line that reaches another irregular vertex within three edges is laid first,
//!   whole; then the others, irregular vertices in increasing number, each stopping at the first
//!   vertex already on a line, at an irregular vertex or at the rim (a motorcycle graph, but
//!   traced one at a time);
//! - then neighbouring faces are joined while their union is still a rectangle (a disc with four
//!   corners whose side vertices have four quads or lie on the rim and whose inner vertices have
//!   four quads), faces taken by their lowest quad;
//! - a region that is no rectangle (a ring with no irregular vertex, a torus) is cut along a
//!   straight line through its lowest quad until it is. This last rule is ours: the measured
//!   layout of rings depends on how the cage is numbered and is not yet known.
//!
//! The measured layout differs from this one at some ties (two lines reaching one vertex in the
//! same step, one across the other): on the open grids measured, 55 of 60 laid out alike.
//!
//! **The rim.** An open cage is first given two rings of ghost points outside its rim: across a
//! rim point with one inner edge `2 v - inner`; at a corner of one quad `2 v - a`, `2 v - b` and
//! `4 v - 2 a - 2 b + c`. That makes the rim the cubic B-spline of the rim points with the corners
//! kept, which is what the measured rim is where every rim point has one inner edge. Where a rim
//! point has more inner edges its ghost is `2 v` less the mean of them: not yet measured.
//!
//! **Not tangent-continuous everywhere.** Where two irregular vertices are close, the measured
//! surface (and this one) has small creases between neighbouring quads: their normals differ by
//! up to about 0.3 degrees on the cages measured. A cage whose irregular corners are all of three
//! quads and two edges apart (a cube, a subdivided cube) is G1 everywhere.
//!
//! Every pole is a fixed blend of the cage's points ([`Blend`]), so a fit can take the cage's
//! points as its unknowns.
//!
//! Deviations: none (no upstream). Every loop is bounded by the cage's size.

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

/// The Greville places of a patch's coefficients, in ninths.
const G: [usize; 8] = [0, 1, 2, 4, 5, 7, 8, 9];

/// The coefficient at Greville place `g`.
fn pos(g: usize) -> Result<usize> {
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

/// A cubic Bézier span's four points into the splines with knots 0000 3366 9999 (exact).
const M8: [[f64; 4]; 8] = [
    [1.0, 0.0, 0.0, 0.0],
    [2.0 / 3.0, 1.0 / 3.0, 0.0, 0.0],
    [4.0 / 9.0, 4.0 / 9.0, 1.0 / 9.0, 0.0],
    [4.0 / 27.0, 4.0 / 9.0, 1.0 / 3.0, 2.0 / 27.0],
    [2.0 / 27.0, 1.0 / 3.0, 4.0 / 9.0, 4.0 / 27.0],
    [0.0, 1.0 / 9.0, 4.0 / 9.0, 4.0 / 9.0],
    [0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// The exact L2 projection onto the splines 0000 3366 9999 of `alpha(u) b'(u)`, `b` the edge
/// spline with the given eight coefficients, at the coefficients of Greville places 2 and 4:
/// `[part][row][k]`, `part` 0 for `alpha = 1 - u/9` and 1 for `alpha = u/9` (exact rationals,
/// from the splines' Gram matrix).
const PROJ: [[[f64; 8]; 2]; 2] = [
    [
        [
            0.0,
            -63724.0 / 235971.0,
            -11171.0 / 157314.0,
            252353.0 / 471942.0,
            -320.0 / 1503.0,
            9088.0 / 235971.0,
            -4544.0 / 235971.0,
            0.0,
        ],
        [
            0.0,
            30188.0 / 235971.0,
            -146971.0 / 471942.0,
            -76013.0 / 471942.0,
            560.0 / 1503.0,
            -13232.0 / 235971.0,
            6616.0 / 235971.0,
            0.0,
        ],
    ],
    [
        [
            0.0,
            -31862.0 / 235971.0,
            1254.0 / 26219.0,
            320.0 / 1503.0,
            -38752.0 / 235971.0,
            18176.0 / 235971.0,
            -9088.0 / 235971.0,
            0.0,
        ],
        [
            0.0,
            15094.0 / 235971.0,
            -56407.0 / 235971.0,
            -59.0 / 1503.0,
            63808.0 / 235971.0,
            -26464.0 / 235971.0,
            13232.0 / 235971.0,
            0.0,
        ],
    ],
];

/// `sigma` for three quads at a corner (the measured value; otherwise `1 / (4 lambda)`).
const SIGMA3: f64 = 0.53;

/// Weights under this are dropped from a blend.
const NEGLIGIBLE: f64 = 1e-15;

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

/// `a + s (b - c)`.
fn add_diff(a: &Blend, s: f64, b: &Blend, c: &Blend) -> Blend {
    lin(&[(1.0, a), (s, b), (-s, c)])
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Axis {
    U,
    V,
}

impl Axis {
    fn flip(self) -> Self {
        match self {
            Axis::U => Axis::V,
            Axis::V => Axis::U,
        }
    }
}

/// A quad mesh with its corners' blends, rings and edges.
struct Mesh {
    pos: Vec<Blend>,
    faces: Vec<[usize; 4]>,
    /// (face, vertex) -> which corner of the face the vertex is.
    corner: BTreeMap<(usize, usize), usize>,
    /// Directed edge (from, to) as a face runs along it -> (face, side): side k runs from corner
    /// k to corner k + 1.
    along: BTreeMap<(usize, usize), (usize, usize)>,
    /// The faces round each vertex, each face's edge back to its previous corner being the next
    /// face's edge on to its next corner; `None` on the rim.
    ring: Vec<Option<Vec<usize>>>,
    /// The faces at each vertex, in face order.
    inc: Vec<Vec<usize>>,
}

impl Mesh {
    fn new(pos: Vec<Blend>, faces: Vec<[usize; 4]>) -> Result<Self> {
        let n = pos.len();
        let mut corner = BTreeMap::new();
        let mut along = BTreeMap::new();
        let mut inc: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (f, q) in faces.iter().enumerate() {
            for (c, &v) in q.iter().enumerate() {
                if v >= n {
                    return Err(bad("a quad names a point the cage does not have"));
                }
                if corner.insert((f, v), c).is_some() {
                    return Err(bad("a quad names one point twice"));
                }
                within(inc.get_mut(v))?.push(f);
                let w = *within(q.get((c + 1) % 4))?;
                if along.insert((v, w), (f, c)).is_some() {
                    return Err(bad(
                        "two quads run the same way along one edge: the quads must all face one way, two to an edge",
                    ));
                }
            }
        }
        let mut m = Mesh {
            pos,
            faces,
            corner,
            along,
            ring: Vec::new(),
            inc,
        };
        let mut ring = Vec::new();
        for v in 0..n {
            ring.push(m.make_ring(v)?);
        }
        m.ring = ring;
        Ok(m)
    }

    fn vert(&self, f: usize, c: usize) -> Result<usize> {
        within(self.faces.get(f).and_then(|q| q.get(c % 4)).copied())
    }

    fn corner_of(&self, f: usize, v: usize) -> Result<usize> {
        within(self.corner.get(&(f, v)).copied())
    }

    fn make_ring(&self, v: usize) -> Result<Option<Vec<usize>>> {
        let fs = within(self.inc.get(v))?;
        let Some(&start) = fs.first() else {
            return Ok(None);
        };
        let mut by_u = BTreeMap::new();
        for &f in fs {
            let c = self.corner_of(f, v)?;
            if by_u.insert(self.vert(f, c + 1)?, f).is_some() {
                return Ok(None);
            }
        }
        let mut order = vec![start];
        loop {
            let f = *within(order.last())?;
            let c = self.corner_of(f, v)?;
            let prev = self.vert(f, c + 3)?;
            let Some(&nf) = by_u.get(&prev) else {
                return Ok(None);
            };
            if nf == start {
                break;
            }
            order.push(nf);
            if order.len() > fs.len() {
                return Ok(None);
            }
        }
        Ok((order.len() == fs.len()).then_some(order))
    }

    fn ring_of(&self, v: usize) -> Option<&Vec<usize>> {
        self.ring.get(v).and_then(Option::as_ref)
    }

    /// The number of quads round an inner vertex; `None` on the rim.
    fn valence(&self, v: usize) -> Option<usize> {
        self.ring_of(v).map(Vec::len)
    }

    /// An inner vertex's spokes (each face's edge on to its next corner) and diagonals, ring order.
    fn one_ring(&self, v: usize) -> Result<(Vec<usize>, Vec<usize>, &[usize])> {
        let faces = within(self.ring_of(v))?;
        let mut spokes = Vec::new();
        let mut diags = Vec::new();
        for &f in faces {
            let c = self.corner_of(f, v)?;
            spokes.push(self.vert(f, c + 1)?);
            diags.push(self.vert(f, c + 2)?);
        }
        Ok((spokes, diags, faces))
    }

    fn pos_of(&self, v: usize) -> Result<&Blend> {
        within(self.pos.get(v))
    }
}

/// `i` taken round a ring of `n`.
fn round(i: usize, n: usize) -> usize {
    i % n.max(1)
}

/// One ring of ghost points outside an open mesh's rim (see the module doc).
fn extend(m: &Mesh) -> Result<(Vec<Blend>, Vec<[usize; 4]>)> {
    let mut edges: BTreeMap<(usize, usize), usize> = BTreeMap::new();
    for &(a, b) in m.along.keys() {
        *edges.entry((a.min(b), a.max(b))).or_insert(0) += 1;
    }
    // rim edges as their one face runs along them
    let rim: Vec<(usize, usize)> = m
        .along
        .keys()
        .filter(|&&(a, b)| !m.along.contains_key(&(b, a)))
        .copied()
        .collect();
    let mut on_rim: BTreeSet<usize> = BTreeSet::new();
    for &(a, b) in &rim {
        on_rim.insert(a);
        on_rim.insert(b);
    }
    let mut pos = m.pos.clone();
    let mut faces = m.faces.clone();
    let mut across: BTreeMap<usize, usize> = BTreeMap::new();
    let mut corners: BTreeMap<usize, (usize, usize, usize, usize)> = BTreeMap::new();
    for &v in &on_rim {
        let fs = within(m.inc.get(v))?;
        if fs.len() == 1 {
            let f = *within(fs.first())?;
            let c = m.corner_of(f, v)?;
            let (a, cc, b) = (m.vert(f, c + 1)?, m.vert(f, c + 2)?, m.vert(f, c + 3)?);
            let (pv, pa, pb, pc) = (m.pos_of(v)?, m.pos_of(a)?, m.pos_of(b)?, m.pos_of(cc)?);
            let ga = pos.len();
            pos.push(lin(&[(2.0, pv), (-1.0, pa)]));
            let gb = pos.len();
            pos.push(lin(&[(2.0, pv), (-1.0, pb)]));
            let gd = pos.len();
            pos.push(lin(&[(4.0, pv), (-2.0, pa), (-2.0, pb), (1.0, pc)]));
            corners.insert(v, (ga, gb, a, gd));
            continue;
        }
        let inner: Vec<usize> = edges
            .iter()
            .filter(|&(&(a, b), &k)| k == 2 && (a == v || b == v))
            .map(|(&(a, b), _)| if a == v { b } else { a })
            .collect();
        if inner.is_empty() {
            return Err(bad("two rims of the cage meet at one point"));
        }
        let share = -1.0 / inner.len() as f64;
        let mut terms = vec![(2.0, m.pos_of(v)?)];
        for &u in &inner {
            terms.push((share, m.pos_of(u)?));
        }
        across.insert(v, pos.len());
        pos.push(lin(&terms));
    }
    let ghost = |v: usize, other: usize| -> Result<usize> {
        if let Some(&(ga, gb, a, _)) = corners.get(&v) {
            return Ok(if other == a { gb } else { ga });
        }
        within(across.get(&v).copied())
    };
    for &(a, b) in &rim {
        faces.push([b, a, ghost(a, b)?, ghost(b, a)?]);
    }
    for (&v, &(ga, gb, _, gd)) in &corners {
        faces.push([v, ga, gd, gb]);
    }
    Ok((pos, faces))
}

/// Corner `c`'s place (i, j) (coefficient positions 0..8, u along the edge to the next corner)
/// as the face's own (a, b).
fn gidx(c: usize, i: usize, j: usize) -> Result<(usize, usize)> {
    let r = |x: usize| within(7usize.checked_sub(x));
    Ok(match c % 4 {
        0 => (i, j),
        1 => (r(j)?, i),
        2 => (r(i)?, r(j)?),
        _ => (j, r(i)?),
    })
}

/// The patches of the quads: 8 x 8 blends each, (a, b) at `a * 8 + b`.
struct Patches<'m> {
    m: &'m Mesh,
    b: BTreeMap<usize, Vec<Blend>>,
    q: BTreeMap<usize, Vec<Blend>>,
    irregular: BTreeMap<usize, bool>,
}

type Key = (usize, usize, Axis, usize);

impl Patches<'_> {
    fn grid(&self, f: usize) -> Result<&Vec<Blend>> {
        within(self.b.get(&f))
    }

    fn l(&self, f: usize, c: usize, gi: usize, gj: usize) -> Result<&Blend> {
        let (a, b) = gidx(c, pos(gi)?, pos(gj)?)?;
        within(self.grid(f)?.get(a * 8 + b))
    }

    fn set_l(&mut self, f: usize, c: usize, gi: usize, gj: usize, val: Blend) -> Result<()> {
        let (a, b) = gidx(c, pos(gi)?, pos(gj)?)?;
        *within(within(self.b.get_mut(&f))?.get_mut(a * 8 + b))? = val;
        Ok(())
    }

    /// In the frame of corner `c`'s edge along `ax`: `gi` along the edge, `gj` into the face.
    fn e(&self, f: usize, c: usize, ax: Axis, gi: usize, gj: usize) -> Result<&Blend> {
        match ax {
            Axis::U => self.l(f, c, gi, gj),
            Axis::V => self.l(f, c, gj, gi),
        }
    }

    fn set_e(
        &mut self,
        f: usize,
        c: usize,
        ax: Axis,
        gi: usize,
        gj: usize,
        val: Blend,
    ) -> Result<()> {
        match ax {
            Axis::U => self.set_l(f, c, gi, gj, val),
            Axis::V => self.set_l(f, c, gj, gi, val),
        }
    }

    fn other_end(c: usize, ax: Axis) -> usize {
        match ax {
            Axis::U => (c + 1) % 4,
            Axis::V => (c + 3) % 4,
        }
    }

    /// The face across corner `c`'s edge along `ax`, its corner at the same vertex and its axis.
    fn neighbour(&self, f: usize, c: usize, ax: Axis) -> Result<(usize, usize, Axis)> {
        let v = self.m.vert(f, c)?;
        let ring = within(self.m.ring_of(v))?;
        let n = ring.len();
        let k = within(ring.iter().position(|&g| g == f))?;
        let nf = match ax {
            Axis::U => *within(ring.get(round(k + n - 1, n)))?,
            Axis::V => *within(ring.get(round(k + 1, n)))?,
        };
        Ok((nf, self.m.corner_of(nf, v)?, ax.flip()))
    }

    fn ends(&self, f: usize, c: usize, ax: Axis) -> Result<(usize, usize)> {
        let v0 = self.m.vert(f, c)?;
        let v1 = self.m.vert(f, Self::other_end(c, ax))?;
        Ok((within(self.m.valence(v0))?, within(self.m.valence(v1))?))
    }

    fn is_irregular(&self, f: usize) -> bool {
        self.irregular.get(&f).copied().unwrap_or(false)
    }

    /// Every face round this one's corners was built.
    fn complete(&self, f: usize) -> Result<bool> {
        for c in 0..4 {
            let v = self.m.vert(f, c)?;
            let Some(ring) = self.m.ring_of(v) else {
                return Ok(false);
            };
            if ring.iter().any(|g| !self.b.contains_key(g)) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    // ---- stage 1 ---------------------------------------------------------------------------

    fn corner_bezier(&self, f: usize, c: usize) -> Result<[Blend; 4]> {
        let v = self.m.vert(f, c)?;
        let (sp, dg, faces) = self.m.one_ring(v)?;
        let n = sp.len();
        let k = within(faces.iter().position(|&g| g == f))?;
        let p = |i: usize| self.m.pos_of(*within(sp.get(round(i, n)))?);
        let d = |i: usize| self.m.pos_of(*within(dg.get(round(i, n)))?);
        let pv = self.m.pos_of(v)?;
        let nf = n as f64;
        let mut terms = vec![(nf / (nf + 5.0), pv)];
        for i in 0..n {
            terms.push((4.0 / (nf * (nf + 5.0)), p(i)?));
            terms.push((1.0 / (nf * (nf + 5.0)), d(i)?));
        }
        let q00 = lin(&terms);
        let km = k + n - 1;
        let q10 = lin(&[
            (8.0 / 18.0, pv),
            (4.0 / 18.0, p(k)?),
            (2.0 / 18.0, p(k + 1)?),
            (2.0 / 18.0, p(km)?),
            (1.0 / 18.0, d(k)?),
            (1.0 / 18.0, d(km)?),
        ]);
        let q01 = lin(&[
            (8.0 / 18.0, pv),
            (4.0 / 18.0, p(k + 1)?),
            (2.0 / 18.0, p(k + 2)?),
            (2.0 / 18.0, p(k)?),
            (1.0 / 18.0, d(k + 1)?),
            (1.0 / 18.0, d(k)?),
        ]);
        let q11 = lin(&[
            (4.0 / 9.0, pv),
            (2.0 / 9.0, p(k)?),
            (2.0 / 9.0, p(k + 1)?),
            (1.0 / 9.0, d(k)?),
        ]);
        Ok([q00, q10, q01, q11])
    }

    fn stage1(&mut self, faces: &[usize]) -> Result<()> {
        for &f in faces {
            let mut q: Vec<Blend> = vec![Vec::new(); 16];
            for c in 0..4 {
                let [q00, q10, q01, q11] = self.corner_bezier(f, c)?;
                for ((i, j), val) in [((0, 0), q00), ((1, 0), q10), ((0, 1), q01), ((1, 1), q11)] {
                    let (a, b) = match c {
                        0 => (i, j),
                        1 => (3 - j, i),
                        2 => (3 - i, 3 - j),
                        _ => (j, 3 - i),
                    };
                    *within(q.get_mut(a * 4 + b))? = val;
                }
            }
            let mut b = Vec::new();
            for a in 0..8 {
                for bb in 0..8 {
                    let ra = within(M8.get(a))?;
                    let rb = within(M8.get(bb))?;
                    let mut terms = Vec::new();
                    for (i, &wa) in ra.iter().enumerate() {
                        for (j, &wb) in rb.iter().enumerate() {
                            if wa * wb != 0.0 {
                                terms.push((wa * wb, within(q.get(i * 4 + j))?));
                            }
                        }
                    }
                    b.push(lin(&terms));
                }
            }
            let mut irr = false;
            for c in 0..4 {
                irr |= self.m.valence(self.m.vert(f, c)?) != Some(4);
            }
            self.irregular.insert(f, irr);
            self.q.insert(f, q);
            // stored a * 8 + b: `b` was built with a outer, b inner
            self.b.insert(f, b);
        }
        Ok(())
    }

    // ---- stage 2: tangents and twists -------------------------------------------------------

    /// `b10 - b00` for the face at ring position `k` of `v`, along spoke `along`.
    fn tangent(&self, v: usize, along: usize) -> Result<Blend> {
        let (sp, dg, _) = self.m.one_ring(v)?;
        let n = sp.len();
        let nf = n as f64;
        let cn = (2.0 * std::f64::consts::PI / nf).cos();
        let lam = (cn + 5.0 + ((cn + 9.0) * (cn + 1.0)).sqrt()) / 16.0;
        let om = 16.0 * lam - 4.0;
        let sig = if n == 3 { SIGMA3 } else { 1.0 / (4.0 * lam) };
        let th = 2.0 * std::f64::consts::PI / nf;
        let scale = sig / (3.0 * (2.0 + om)) / 3.0;
        let mut terms = Vec::new();
        for l in 0..n {
            let a = th * (l as f64 - along as f64);
            let b = th * (l as f64 + 1.0 - along as f64);
            terms.push((scale * om * a.cos(), self.m.pos_of(*within(sp.get(l))?)?));
            terms.push((
                scale * (a.cos() + b.cos()),
                self.m.pos_of(*within(dg.get(l))?)?,
            ));
        }
        Ok(lin(&terms))
    }

    fn stage2(&mut self, faces: &[usize]) -> Result<()> {
        for &f in faces {
            if !self.is_irregular(f) {
                continue;
            }
            for c in 0..4 {
                let v = self.m.vert(f, c)?;
                let n = within(self.m.valence(v))?;
                let b00 = self.l(f, c, 0, 0)?.clone();
                if n != 4 {
                    let ring = within(self.m.ring_of(v))?;
                    let k = within(ring.iter().position(|&g| g == f))?;
                    let tu = self.tangent(v, k)?;
                    let tv = self.tangent(v, k + 1)?;
                    self.set_l(f, c, 1, 0, lin(&[(1.0, &b00), (1.0, &tu)]))?;
                    self.set_l(f, c, 0, 1, lin(&[(1.0, &b00), (1.0, &tv)]))?;
                }
                // q11 of corner c on the 4 x 4 grid: (1, 1), (2, 1), (2, 2), (1, 2)
                let qi = *within([5usize, 9, 10, 6].get(c))?;
                let q11 = within(within(self.q.get(&f))?.get(qi))?.clone();
                let b10 = self.l(f, c, 1, 0)?.clone();
                let b01 = self.l(f, c, 0, 1)?.clone();
                let b11 = lin(&[
                    (6.0 / 9.0, &b10),
                    (6.0 / 9.0, &b01),
                    (-4.0 / 9.0, &b00),
                    (1.0 / 9.0, &q11),
                ]);
                self.set_l(f, c, 1, 1, b11)?;
            }
        }
        Ok(())
    }

    // ---- stage 3: the boundary --------------------------------------------------------------

    fn jobs(&self, faces: &[usize]) -> Vec<(usize, usize, Axis)> {
        let mut out = Vec::new();
        for &f in faces {
            if !self.is_irregular(f) {
                continue;
            }
            for c in 0..4 {
                out.push((f, c, Axis::U));
                out.push((f, c, Axis::V));
            }
        }
        out
    }

    fn stage3(&mut self, faces: &[usize]) -> Result<()> {
        let jobs = self.jobs(faces);
        let mut new: Vec<(Key, Blend)> = Vec::new();
        for &(f, c, ax) in &jobs {
            let (n0, nk) = self.ends(f, c, ax)?;
            if n0 == 4 {
                continue;
            }
            let l0 = 2.0 * (2.0 * std::f64::consts::PI / n0 as f64).cos();
            let l1 = if nk == 4 {
                2.0 * l0 / 3.0
            } else {
                (2.0 * l0 - 2.0 * (2.0 * std::f64::consts::PI / nk as f64).cos()) / 3.0
            };
            let (nf, nc, nax) = self.neighbour(f, c, ax)?;
            let b00 = self.e(f, c, ax, 0, 0)?;
            let b10 = self.e(f, c, ax, 1, 0)?;
            let b11 = self.e(f, c, ax, 1, 1)?;
            let b11n = self.e(nf, nc, nax, 1, 1)?;
            // b10 + (3 (b11 + b11' - 2 b10) - l1 (b10 - b00)) / (2 l0)
            let s = 1.0 / (2.0 * l0);
            let val = lin(&[
                (1.0 - 6.0 * s - l1 * s, b10),
                (3.0 * s, b11),
                (3.0 * s, b11n),
                (l1 * s, b00),
            ]);
            new.push(((f, c, ax, 2), val));
        }
        for ((f, c, ax, g), val) in new {
            self.set_e(f, c, ax, g, 0, val)?;
        }
        let mut new: Vec<(Key, Blend)> = Vec::new();
        for &(f, c, ax) in &jobs {
            let (n0, nk) = self.ends(f, c, ax)?;
            if n0 == 4 && nk == 4 {
                continue;
            }
            let g2 = self.e(f, c, ax, 2, 0)?;
            let g8 = self.e(f, c, ax, 8, 0)?;
            let g7 = self.e(f, c, ax, 7, 0)?;
            let g1 = self.e(f, c, ax, 1, 0)?;
            new.push((
                (f, c, ax, 4),
                lin(&[
                    (4.0 / 3.0, g2),
                    (-1.0 / 3.0, g8),
                    (2.0 / 3.0, g7),
                    (-2.0 / 3.0, g1),
                ]),
            ));
        }
        for ((f, c, ax, g), val) in new {
            self.set_e(f, c, ax, g, 0, val)?;
        }
        Ok(())
    }

    // ---- stage 4: the first inner row -------------------------------------------------------

    fn stage4(&mut self, faces: &[usize]) -> Result<()> {
        let built: Vec<usize> = self.b.keys().copied().collect();
        let mut pre: BTreeMap<Key, Blend> = BTreeMap::new();
        for (f, c, ax) in self.jobs(&built) {
            let (n0, nk) = self.ends(f, c, ax)?;
            if n0 == 4 && nk == 4 {
                continue;
            }
            let e01 = self.e(f, c, ax, 0, 1)?;
            let e11 = self.e(f, c, ax, 1, 1)?;
            let e81 = self.e(f, c, ax, 8, 1)?;
            let e91 = self.e(f, c, ax, 9, 1)?;
            pre.insert(
                (f, c, ax, 2),
                lin(&[
                    (-4.0 / 9.0, e01),
                    (4.0 / 3.0, e11),
                    (1.0 / 3.0, e81),
                    (-2.0 / 9.0, e91),
                ]),
            );
            pre.insert(
                (f, c, ax, 4),
                lin(&[
                    (-20.0 / 27.0, e01),
                    (4.0 / 3.0, e11),
                    (1.0, e81),
                    (-16.0 / 27.0, e91),
                ]),
            );
        }
        let mut new: Vec<(Key, Blend)> = Vec::new();
        for (f, c, ax) in self.jobs(faces) {
            let (n0, nk) = self.ends(f, c, ax)?;
            if n0 == 4 && nk == 4 {
                continue;
            }
            let l0 = if n0 == 4 {
                0.0
            } else {
                2.0 * (2.0 * std::f64::consts::PI / n0 as f64).cos()
            };
            let l3 = if nk == 4 {
                0.0
            } else {
                -2.0 * (2.0 * std::f64::consts::PI / nk as f64).cos()
            };
            let (nf, nc, nax) = self.neighbour(f, c, ax)?;
            for (row, gi) in [(0usize, 2usize), (1, 4)] {
                let mut terms = vec![(1.0, self.e(f, c, ax, gi, 0)?)];
                for (k, &g) in G.iter().enumerate() {
                    let w0 = *within(PROJ.first().and_then(|p| p.get(row)).and_then(|r| r.get(k)))?;
                    let w3 = *within(PROJ.get(1).and_then(|p| p.get(row)).and_then(|r| r.get(k)))?;
                    let w = 0.5 * (l0 * w0 + l3 * w3);
                    if w != 0.0 {
                        terms.push((w, self.e(f, c, ax, g, 0)?));
                    }
                }
                let mine = within(pre.get(&(f, c, ax, gi)))?;
                let theirs = within(pre.get(&(nf, nc, nax, gi)))?;
                terms.push((0.5, mine));
                terms.push((-0.5, theirs));
                new.push(((f, c, ax, gi), lin(&terms)));
            }
        }
        for ((f, c, ax, g), val) in new {
            self.set_e(f, c, ax, g, 1, val)?;
        }
        Ok(())
    }

    // ---- stage 5: the inside ----------------------------------------------------------------

    fn stage5(&mut self, faces: &[usize]) -> Result<()> {
        let irr: Vec<usize> = faces
            .iter()
            .copied()
            .filter(|&f| self.is_irregular(f))
            .collect();
        let mut new: Vec<((usize, usize, usize, usize), Blend)> = Vec::new();
        for &f in &irr {
            for c in 0..4 {
                let l = |i, j| self.l(f, c, i, j);
                let val = lin(&[
                    (-10.0 / 27.0, l(0, 4)?),
                    (2.0 / 3.0, l(1, 4)?),
                    (0.5, l(8, 4)?),
                    (-8.0 / 27.0, l(9, 4)?),
                    (-10.0 / 27.0, l(4, 0)?),
                    (2.0 / 3.0, l(4, 1)?),
                    (0.5, l(4, 8)?),
                    (-8.0 / 27.0, l(4, 9)?),
                ]);
                new.push(((f, c, 4, 4), val));
            }
        }
        self.apply(new)?;
        let mut new = Vec::new();
        for &f in &irr {
            for c in 0..4 {
                let l = |i, j| self.l(f, c, i, j);
                new.push((
                    (f, c, 4, 2),
                    lin(&[(0.5, l(4, 1)?), (1.0, l(4, 4)?), (-0.5, l(4, 5)?)]),
                ));
                new.push((
                    (f, c, 2, 4),
                    lin(&[(0.5, l(1, 4)?), (1.0, l(4, 4)?), (-0.5, l(5, 4)?)]),
                ));
            }
        }
        self.apply(new)?;
        let mut new = Vec::new();
        for &f in &irr {
            for c in 0..4 {
                let l = |i, j| self.l(f, c, i, j);
                let val = lin(&[
                    (0.25, l(1, 2)?),
                    (0.5, l(4, 2)?),
                    (-0.25, l(5, 2)?),
                    (0.25, l(2, 1)?),
                    (0.5, l(2, 4)?),
                    (-0.25, l(2, 5)?),
                ]);
                new.push(((f, c, 2, 2), val));
            }
        }
        self.apply(new)
    }

    fn apply(&mut self, new: Vec<((usize, usize, usize, usize), Blend)>) -> Result<()> {
        for ((f, c, i, j), val) in new {
            self.set_l(f, c, i, j, val)?;
        }
        Ok(())
    }
}

// ---- the layout ---------------------------------------------------------------------------------

type Edge = (usize, usize);

fn edge(a: usize, b: usize) -> Edge {
    (a.min(b), a.max(b))
}

/// The vertex straight on from `prev` through `cur` (a vertex of four quads).
fn straight_on(m: &Mesh, prev: usize, cur: usize) -> Result<usize> {
    let (sp, _, _) = m.one_ring(cur)?;
    let k = within(sp.iter().position(|&s| s == prev))?;
    Ok(*within(sp.get((k + 2) % 4))?)
}

/// The edges on the layout's lines (see the module doc).
fn lines(m: &Mesh) -> Result<BTreeSet<Edge>> {
    let n = m.pos.len();
    let inner = |v: usize| m.valence(v).is_some();
    let regular = |v: usize| m.valence(v) == Some(4);
    let irr: Vec<usize> = (0..n).filter(|&v| inner(v) && !regular(v)).collect();
    let mut occupied: BTreeSet<usize> = (0..n).filter(|&v| !inner(v)).collect();
    occupied.extend(irr.iter().copied());
    let bound = 4 * n + 4;
    let mut conn = Vec::new();
    let mut rest = Vec::new();
    for &x in &irr {
        let (sp, _, _) = m.one_ring(x)?;
        for &s in &sp {
            let mut p = vec![x, s];
            while p.len() <= bound {
                let cur = *within(p.last())?;
                if !regular(cur) {
                    break;
                }
                let prev = *within(p.get(p.len() - 2))?;
                let next = straight_on(m, prev, cur)?;
                p.push(next);
                if next == x {
                    break;
                }
            }
            let end = *within(p.last())?;
            if inner(end) && !regular(end) && p.len() <= 4 {
                conn.push(p);
            } else {
                rest.push((x, s));
            }
        }
    }
    let mut lines = BTreeSet::new();
    for p in &conn {
        for w in p.windows(2) {
            if let [a, b] = w {
                lines.insert(edge(*a, *b));
            }
        }
        occupied.extend(p.iter().copied());
    }
    for (x, s) in rest {
        let (mut prev, mut cur) = (x, s);
        for _ in 0..bound {
            if !lines.insert(edge(prev, cur)) {
                break;
            }
            if !occupied.insert(cur) {
                break;
            }
            let next = straight_on(m, prev, cur)?;
            prev = cur;
            cur = next;
        }
    }
    Ok(lines)
}

/// The quads joined across every inner edge not on `lines`, each quad's group named by the
/// lowest quad in it.
fn groups(m: &Mesh, lines: &BTreeSet<Edge>, of: &[usize]) -> Result<BTreeMap<usize, usize>> {
    let set: BTreeSet<usize> = of.iter().copied().collect();
    let mut parent: BTreeMap<usize, usize> = of.iter().map(|&f| (f, f)).collect();
    let find = |parent: &BTreeMap<usize, usize>, mut x: usize| -> Result<usize> {
        for _ in 0..=parent.len() {
            let p = *within(parent.get(&x))?;
            if p == x {
                return Ok(x);
            }
            x = p;
        }
        Err(fail("a group's chain does not end"))
    };
    for (&(a, b), &(f, _)) in &m.along {
        let Some(&(g, _)) = m.along.get(&(b, a)) else {
            continue;
        };
        if !set.contains(&f) || !set.contains(&g) || lines.contains(&edge(a, b)) {
            continue;
        }
        let (rf, rg) = (find(&parent, f)?, find(&parent, g)?);
        if rf != rg {
            parent.insert(rf.max(rg), rf.min(rg));
        }
    }
    let mut out = BTreeMap::new();
    let mut low: BTreeMap<usize, usize> = BTreeMap::new();
    for &f in of {
        let r = find(&parent, f)?;
        let e = low.entry(r).or_insert(f);
        *e = (*e).min(f);
    }
    for &f in of {
        out.insert(f, *within(low.get(&find(&parent, f)?))?);
    }
    Ok(out)
}

/// The quads form a rectangle (see the module doc).
fn is_rect(m: &Mesh, quads: &BTreeSet<usize>) -> Result<bool> {
    let mut inc: BTreeMap<usize, usize> = BTreeMap::new();
    let mut edges = BTreeSet::new();
    for &f in quads {
        for c in 0..4 {
            let v = m.vert(f, c)?;
            *inc.entry(v).or_insert(0) += 1;
            edges.insert(edge(v, m.vert(f, c + 1)?));
        }
    }
    let mut corners = 0;
    for (&v, &k) in &inc {
        let tot = m.valence(v);
        if tot == Some(k) {
            if k != 4 {
                return Ok(false);
            }
            continue;
        }
        match k {
            1 => corners += 1,
            2 => {
                if tot.is_some_and(|t| t != 4) {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
    }
    Ok(corners == 4 && inc.len() + quads.len() == edges.len() + 1)
}

fn members(label: &BTreeMap<usize, usize>) -> BTreeMap<usize, BTreeSet<usize>> {
    let mut out: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    for (&f, &l) in label {
        out.entry(l).or_default().insert(f);
    }
    out
}

/// Join neighbouring groups while their union is a rectangle, groups by their lowest quad.
fn merge(m: &Mesh, label: &mut BTreeMap<usize, usize>) -> Result<()> {
    let total = label.len();
    for _ in 0..=total {
        let groups = members(label);
        let mut joined = false;
        'outer: for (&l, quads) in &groups {
            let mut nbrs = BTreeSet::new();
            for &f in quads {
                for c in 0..4 {
                    let (a, b) = (m.vert(f, c)?, m.vert(f, c + 1)?);
                    if let Some(&(g, _)) = m.along.get(&(b, a)) {
                        if let Some(&lg) = label.get(&g) {
                            if lg != l {
                                nbrs.insert(lg);
                            }
                        }
                    }
                }
            }
            for n2 in nbrs {
                let other = within(groups.get(&n2))?;
                let u: BTreeSet<usize> = quads.union(other).copied().collect();
                if is_rect(m, &u)? {
                    let low = l.min(n2);
                    for f in u {
                        label.insert(f, low);
                    }
                    joined = true;
                    break 'outer;
                }
            }
        }
        if !joined {
            return Ok(());
        }
    }
    Ok(())
}

/// The quads of `part` by how far each is from `from` across edges not on `lines` (farthest first,
/// then by number).
fn by_distance(
    m: &Mesh,
    part: &BTreeSet<usize>,
    lines: &BTreeSet<Edge>,
    from: usize,
) -> Result<Vec<usize>> {
    let mut dist: BTreeMap<usize, usize> = BTreeMap::new();
    dist.insert(from, 0);
    let mut front = vec![from];
    let mut d = 0;
    while !front.is_empty() {
        d += 1;
        let mut next = Vec::new();
        for &f in &front {
            for c in 0..4 {
                let (a, b) = (m.vert(f, c)?, m.vert(f, c + 1)?);
                if lines.contains(&edge(a, b)) {
                    continue;
                }
                if let Some(&(g, _)) = m.along.get(&(b, a)) {
                    if part.contains(&g) && !dist.contains_key(&g) {
                        dist.insert(g, d);
                        next.push(g);
                    }
                }
            }
        }
        front = next;
    }
    let mut out: Vec<usize> = part.iter().copied().collect();
    out.sort_by_key(|f| (std::cmp::Reverse(dist.get(f).copied().unwrap_or(0)), *f));
    Ok(out)
}

/// Lay a straight line through side `side` of quad `q`, both ways, inside `part`; how many edges
/// it added.
fn cut(
    m: &Mesh,
    part: &BTreeSet<usize>,
    lines: &mut BTreeSet<Edge>,
    q: usize,
    side: usize,
) -> Result<usize> {
    let start = (m.vert(q, side)?, m.vert(q, side + 1)?);
    let mut added = usize::from(lines.insert(edge(start.0, start.1)));
    for (mut prev, mut cur) in [start, (start.1, start.0)] {
        for _ in 0..m.pos.len() {
            if m.valence(cur) != Some(4) {
                break;
            }
            let next = straight_on(m, prev, cur)?;
            let inside = [m.along.get(&(cur, next)), m.along.get(&(next, cur))]
                .iter()
                .all(|x| x.is_some_and(|&(f, _)| part.contains(&f)));
            if !inside || !lines.insert(edge(cur, next)) {
                break;
            }
            added += 1;
            prev = cur;
            cur = next;
        }
    }
    Ok(added)
}

/// Cut every group that is no rectangle (a ring, a torus) along straight lines until each part is
/// one: through the part's lowest quad, else through the quad farthest from it.
fn cut_rings(
    m: &Mesh,
    label: &mut BTreeMap<usize, usize>,
    lines: &mut BTreeSet<Edge>,
) -> Result<()> {
    for (_, quads) in members(label) {
        let mut done: Vec<BTreeSet<usize>> = Vec::new();
        let mut todo: Vec<BTreeSet<usize>> = vec![quads];
        let mut cuts = 0;
        while let Some(part) = todo.pop() {
            if is_rect(m, &part)? || cuts >= MOST_CUTS {
                done.push(part);
                continue;
            }
            let low = *within(part.iter().next())?;
            let mut added = 0;
            let far = by_distance(m, &part, lines, low)?;
            'tries: for q in std::iter::once(low).chain(far) {
                for side in 0..2 {
                    added = cut(m, &part, lines, q, side)?;
                    if added > 0 {
                        break 'tries;
                    }
                }
            }
            cuts += 1;
            if added == 0 {
                done.push(part);
                continue;
            }
            let list: Vec<usize> = part.iter().copied().collect();
            todo.extend(members(&groups(m, lines, &list)?).into_values());
        }
        for part in done {
            let rect = is_rect(m, &part)?;
            let low = *within(part.iter().next())?;
            for f in part {
                label.insert(f, if rect { low } else { f });
            }
        }
    }
    Ok(())
}

// ---- faces ------------------------------------------------------------------------------------

/// One face of the surface: a rectangle of `size[0]` x `size[1]` quads, a bicubic B-spline with
/// single knots between quads and double knots inside them, `(5 k + 3)` poles for `k` quads each
/// way, u fastest. The parameter runs 0..`size[0]` in u and 0..`size[1]` in v, one per quad.
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

/// The single-knot poles one way from `k` quads' eight coefficients: `(quad, coefficient, weight)`.
fn single_knot_rows(k: usize) -> Vec<Vec<(usize, usize, f64)>> {
    let mut rows = Vec::new();
    for t in 0..k {
        let first = if t == 0 { 0 } else { 2 };
        let last = if t + 1 == k { 7 } else { 5 };
        for i in first..=last {
            rows.push(vec![(t, i, 1.0)]);
        }
        if t + 1 < k {
            rows.push(vec![(t, 6, 2.0), (t, 5, -1.0)]);
        }
    }
    rows
}

fn knots(k: usize) -> Vec<f64> {
    let mut out = vec![0.0; 4];
    for t in 0..k {
        let t0 = t as f64;
        out.extend([
            t0 + 1.0 / 3.0,
            t0 + 1.0 / 3.0,
            t0 + 2.0 / 3.0,
            t0 + 2.0 / 3.0,
        ]);
        if t + 1 < k {
            out.push(t0 + 1.0);
        }
    }
    out.extend([k as f64; 4]);
    out
}

/// The rectangle of a group's quads: `[nu, nv]` and the (quad, rot) at each place, or `None`.
/// A rectangle of quads: its size and the (quad, rot) at each place, u fastest.
type Rect = ([usize; 2], Vec<(usize, u8)>);

fn grid(m: &Mesh, quads: &BTreeSet<usize>) -> Result<Option<Rect>> {
    let Some(&seed) = quads.iter().next() else {
        return Ok(None);
    };
    let mut at: BTreeMap<(i64, i64), (usize, usize)> = BTreeMap::new();
    let mut seen: BTreeMap<usize, (i64, i64)> = BTreeMap::new();
    at.insert((0, 0), (seed, 0));
    seen.insert(seed, (0, 0));
    let mut stack = vec![(0i64, 0i64)];
    let step = [(0i64, -1i64), (1, 0), (0, 1), (-1, 0)];
    while let Some(cell) = stack.pop() {
        let (q, rot) = *within(at.get(&cell))?;
        for (d, &(du, dv)) in step.iter().enumerate() {
            let k = (d + rot) % 4;
            let (a, b) = (m.vert(q, k)?, m.vert(q, k + 1)?);
            let Some(&(g, kg)) = m.along.get(&(b, a)) else {
                continue;
            };
            if !quads.contains(&g) {
                continue;
            }
            let rg = (kg + 6 - d) % 4;
            let nc = (cell.0 + du, cell.1 + dv);
            match (at.get(&nc), seen.get(&g)) {
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
    let u0 = at.keys().map(|c| c.0).min().unwrap_or(0);
    let v0 = at.keys().map(|c| c.1).min().unwrap_or(0);
    let u1 = at.keys().map(|c| c.0).max().unwrap_or(0);
    let v1 = at.keys().map(|c| c.1).max().unwrap_or(0);
    let (nu, nv) = ((u1 - u0 + 1) as usize, (v1 - v0 + 1) as usize);
    if nu * nv != at.len() {
        return Ok(None);
    }
    let mut cells = Vec::new();
    for j in 0..nv as i64 {
        for i in 0..nu as i64 {
            let (q, r) = *within(at.get(&(u0 + i, v0 + j)))?;
            cells.push((q, r as u8));
        }
    }
    Ok(Some(([nu, nv], cells)))
}

/// A face's quads in its own frame, joined by knot removal, as single-knot poles.
fn face(p: &Patches, size: [usize; 2], cells: Vec<(usize, u8)>) -> Result<CageFace> {
    let [nu, nv] = size;
    // each cell's 8 x 8 in the face's frame, (i along u, j along v) at i * 8 + j
    let mut c: Vec<Vec<Blend>> = Vec::new();
    for &(q, rot) in &cells {
        let g = p.grid(q)?;
        let mut cell = Vec::new();
        for i in 0..8 {
            for j in 0..8 {
                let (a, b) = gidx(rot as usize, i, j)?;
                cell.push(within(g.get(a * 8 + b))?.clone());
            }
        }
        c.push(cell);
    }
    let get = |c: &Vec<Vec<Blend>>, cell: usize, i: usize, j: usize| -> Result<Blend> {
        Ok(within(c.get(cell).and_then(|x| x.get(i * 8 + j)))?.clone())
    };
    // joints across u, then across v
    for dir in 0..2 {
        let mut new = Vec::new();
        for jv in 0..nv {
            for iu in 0..nu {
                let here = iu + nu * jv;
                let (there, ok) = if dir == 0 {
                    (here + 1, iu + 1 < nu)
                } else {
                    (here + nu, jv + 1 < nv)
                };
                if !ok {
                    continue;
                }
                for t in 0..8 {
                    let (b0, b2, b2n) = if dir == 0 {
                        (
                            get(&c, here, 7, t)?,
                            get(&c, here, 5, t)?,
                            get(&c, there, 2, t)?,
                        )
                    } else {
                        (
                            get(&c, here, t, 7)?,
                            get(&c, here, t, 5)?,
                            get(&c, there, t, 2)?,
                        )
                    };
                    let b1 = add_diff(&b0, 0.25, &b2, &b2n);
                    let b1n = lin(&[(2.0, &b0), (-1.0, &b1)]);
                    if dir == 0 {
                        new.push((here, 6, t, b1));
                        new.push((there, 1, t, b1n));
                    } else {
                        new.push((here, t, 6, b1));
                        new.push((there, t, 1, b1n));
                    }
                }
            }
        }
        for (cell, i, j, val) in new {
            *within(c.get_mut(cell).and_then(|x| x.get_mut(i * 8 + j)))? = val;
        }
    }
    let ru = single_knot_rows(nu);
    let rv = single_knot_rows(nv);
    let mut poles = Vec::new();
    for row_v in &rv {
        for row_u in &ru {
            let mut held = Vec::new();
            for &(tv, jv, wv) in row_v {
                for &(tu, iu, wu) in row_u {
                    held.push((wu * wv, get(&c, tu + nu * tv, iu, jv)?));
                }
            }
            let terms: Vec<(f64, &Blend)> = held.iter().map(|(w, b)| (*w, b)).collect();
            poles.push(lin(&terms));
        }
    }
    Ok(CageFace {
        size,
        quads: cells,
        knots_u: knots(nu),
        knots_v: knots(nv),
        poles,
    })
}

impl CageSurface {
    /// The surface of the cage with `points` points and these quads, each counter-clockwise seen
    /// from outside, two to an edge at most.
    ///
    /// # Errors
    ///
    /// `InvalidInput` when a quad names a point the cage does not have or one point twice, two
    /// quads run the same way along an edge, two rims meet at one point, or there are no quads.
    pub fn of(points: usize, quads: &[[u32; 4]]) -> Result<CageSurface> {
        if quads.is_empty() {
            return Err(bad("a cage with no quads"));
        }
        let pos: Vec<Blend> = (0..points).map(|i| vec![(i as u32, 1.0)]).collect();
        let faces: Vec<[usize; 4]> = quads.iter().map(|q| q.map(|v| v as usize)).collect();
        let real = Mesh::new(pos, faces)?;
        let nreal = real.faces.len();
        // two rings of ghosts outside an open cage
        let mut work = Mesh::new(real.pos.clone(), real.faces.clone())?;
        for _ in 0..2 {
            if work
                .along
                .keys()
                .all(|&(a, b)| work.along.contains_key(&(b, a)))
            {
                break;
            }
            let (pos, faces) = extend(&work)?;
            work = Mesh::new(pos, faces)?;
        }
        let mut p = Patches {
            m: &work,
            b: BTreeMap::new(),
            q: BTreeMap::new(),
            irregular: BTreeMap::new(),
        };
        let mut ok = Vec::new();
        for f in 0..work.faces.len() {
            let mut all = true;
            for c in 0..4 {
                all &= work.ring_of(work.vert(f, c)?).is_some();
            }
            if all {
                ok.push(f);
            }
        }
        p.stage1(&ok)?;
        let mut faces = Vec::new();
        for &f in &ok {
            if p.complete(f)? {
                faces.push(f);
            }
        }
        if (0..nreal).any(|f| faces.binary_search(&f).is_err()) {
            return Err(fail(
                "a quad of the cage is not surrounded after the ghost rings",
            ));
        }
        p.stage2(&faces)?;
        p.stage3(&faces)?;
        p.stage4(&faces)?;
        p.stage5(&faces)?;
        // the layout, on the cage as given
        let mut lines = lines(&real)?;
        let all: Vec<usize> = (0..nreal).collect();
        let mut label = groups(&real, &lines, &all)?;
        merge(&real, &mut label)?;
        cut_rings(&real, &mut label, &mut lines)?;
        let mut out = Vec::new();
        for (_, quads) in members(&label) {
            match grid(&real, &quads)? {
                Some((size, cells)) => out.push(face(&p, size, cells)?),
                None => {
                    for &q in &quads {
                        out.push(face(&p, [1, 1], vec![(q, 0)])?);
                    }
                }
            }
        }
        Ok(CageSurface { faces: out })
    }
}

#[cfg(test)]
#[path = "quad_cage_tests.rs"]
mod tests;
