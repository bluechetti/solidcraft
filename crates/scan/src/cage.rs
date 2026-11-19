//! A cage: a mesh of four-sided faces whose points are control points, and the smooth patches it
//! stands for, one a face, that meet one another in curvature (G2).
//!
//! - Where four faces meet at every corner of a face, the face's patch is the piece of the uniform
//!   bicubic B-spline whose control points are the sixteen cage points round it. Such patches meet
//!   in curvature exactly, as one B-spline's pieces do.
//! - Where three, five or more faces meet at a corner (an *irregular* corner), bicubic patches
//!   cannot meet in curvature. Each face at such a corner gets a patch of degree 7 from the
//!   kernel (`solidcore_ops::corner_patches`, after Loop and Schaefer's *G2 Tensor Product
//!   Splines over Extraordinary Vertices*, 2008), which meets its neighbours at the corner and
//!   the bicubic faces round them in curvature.
//!
//! Every point of every patch is a fixed blend of cage points ([`CagePatch::blends`]). So a fit
//! of the patches to a scan can take the cage's points as its unknowns, and whatever it finds
//! still meets in curvature.
//!
//! What gets no patch here ([`Patches::left_out`]): a face that touches the cage's open rim, a
//! face with two irregular corners, and a face at an irregular corner whose neighbours are not
//! plain (another irregular corner or the rim within two steps). After SurfaceCraft's Auto
//! Freeform, which solved the same faces as one cage.

use std::collections::HashMap;

use solidcore_ops::corner_patches as corner;
use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;

/// One cubic B-spline span's Bézier points from its four control points, times six.
const TO_BEZIER: [[f64; 4]; 4] = [[1.0, 4.0, 1.0, 0.0], [0.0, 4.0, 2.0, 0.0], [0.0, 2.0, 4.0, 0.0], [0.0, 1.0, 4.0, 1.0]];

/// Why a mesh is no cage, or its patches could not be made.
#[derive(Clone, Debug, PartialEq)]
pub enum CageError {
    /// A face names a point the cage does not have, or the same point twice.
    BadFace(usize),
    /// A point is not a finite number.
    BadPoint(usize),
    /// Two faces run the same way along one edge: the faces do not all face one way, or three
    /// faces share an edge.
    Folded(usize),
    /// The kernel refused the corner patches.
    Corner(String),
}

impl std::fmt::Display for CageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CageError::BadFace(i) => write!(f, "face {i} of the cage names a point it does not have, or one point twice"),
            CageError::BadPoint(i) => write!(f, "point {i} of the cage is not a number"),
            CageError::Folded(i) => {
                write!(f, "face {i} of the cage runs along an edge the same way as another: the faces must all face one way, two to an edge")
            }
            CageError::Corner(why) => write!(f, "the patches at an irregular corner could not be made: {why}"),
        }
    }
}

impl std::error::Error for CageError {}

/// A mesh of four-sided faces, each counter-clockwise seen from outside, two to an edge at most.
#[derive(Clone, Debug)]
pub struct Cage {
    points: Vec<Vec3>,
    faces: Vec<[u32; 4]>,
    /// For each edge as a face runs along it (from, to): that face and which of its corners
    /// `from` is.
    along: HashMap<(u32, u32), (usize, usize)>,
}

/// A face's patch: a Bézier patch of degree `degree` both ways whose point (a, b) is the
/// `a + (degree + 1) b`-th. It is laid on its face as the face is numbered: (0, 0) at the face's
/// first corner, u toward its second and v toward its fourth.
#[derive(Clone, Debug)]
pub struct CagePatch {
    /// The face of the cage this is the patch of.
    pub face: usize,
    /// 3 where four faces meet at each corner, 7 at an irregular corner.
    pub degree: usize,
    /// Each point as a blend of cage points: which, and how much of each.
    pub blends: Vec<Vec<(u32, f64)>>,
}

/// The patches of a cage, and the faces that have none.
#[derive(Clone, Debug, Default)]
pub struct Patches {
    pub patches: Vec<CagePatch>,
    /// The faces with no patch: at the rim, or too near two irregular corners (see the module).
    pub left_out: Vec<usize>,
}

/// The kernel's blends for irregular corners, one for each number of faces met so far: making
/// one takes a moment, so a caller keeps this across cages.
#[derive(Default)]
pub struct CornerBases(HashMap<usize, corner::CornerBasis>);

impl CornerBases {
    fn of(&mut self, faces: usize) -> Result<&corner::CornerBasis, CageError> {
        match self.0.entry(faces) {
            std::collections::hash_map::Entry::Occupied(made) => Ok(made.into_mut()),
            std::collections::hash_map::Entry::Vacant(place) => {
                let basis = corner::corner_basis(faces).map_err(|e| CageError::Corner(e.to_string()))?;
                Ok(place.insert(basis))
            }
        }
    }
}

impl CagePatch {
    /// The patch's points for these cage points (u fastest).
    pub fn points(&self, cage: &[Vec3]) -> Vec<Vec3> {
        self.blends
            .iter()
            .map(|blend| {
                let mut p = Vec3::new(0.0, 0.0, 0.0);
                for &(k, w) in blend {
                    if let Some(q) = cage.get(k as usize) {
                        p.x += w * q.x;
                        p.y += w * q.y;
                        p.z += w * q.z;
                    }
                }
                p
            })
            .collect()
    }

    /// The patch as a surface on the unit square, for these cage points.
    pub fn surface(&self, cage: &[Vec3]) -> Option<NurbsSurface> {
        let count = self.degree + 1;
        let knots: Vec<f64> = std::iter::repeat_n(0.0, count).chain(std::iter::repeat_n(1.0, count)).collect();
        let points = self.points(cage);
        NurbsSurface::new([self.degree; 2], [knots.clone(), knots], &points, &vec![1.0; points.len()]).ok()
    }
}

impl Cage {
    /// The cage of `points` and `faces`.
    pub fn new(points: Vec<Vec3>, faces: Vec<[u32; 4]>) -> Result<Cage, CageError> {
        if let Some(i) = points.iter().position(|p| !p.is_finite()) {
            return Err(CageError::BadPoint(i));
        }
        let mut along = HashMap::with_capacity(4 * faces.len());
        for (f, face) in faces.iter().enumerate() {
            let distinct = (0..4).all(|a| (a + 1..4).all(|b| face[a] != face[b]));
            if !distinct || face.iter().any(|&p| p as usize >= points.len()) {
                return Err(CageError::BadFace(f));
            }
            for k in 0..4 {
                if along.insert((face[k], face[(k + 1) % 4]), (f, k)).is_some() {
                    return Err(CageError::Folded(f));
                }
            }
        }
        Ok(Cage { points, faces, along })
    }

    pub fn points(&self) -> &[Vec3] {
        &self.points
    }

    pub fn faces(&self) -> &[[u32; 4]] {
        &self.faces
    }

    /// The faces round the point `v`, counter-clockwise from the face `start` (of which `v` is
    /// corner `k`), each with which of its corners `v` is; none when they do not go all the way
    /// round (the point is on the rim).
    fn fan(&self, v: u32, start: (usize, usize)) -> Option<Vec<(usize, usize)>> {
        let mut fan = vec![start];
        let mut at = start;
        // No point has more faces round it than the cage has faces.
        for _ in 0..self.faces.len() {
            let before = self.faces.get(at.0)?[(at.1 + 3) % 4];
            let next = *self.along.get(&(v, before))?;
            if next == start {
                return Some(fan);
            }
            fan.push(next);
            at = next;
        }
        None
    }

    /// The point the edge from `a` to `b` runs straight on to, past `b`: `b` must be a plain
    /// point (four faces round it).
    fn straight(&self, fans: &Fans, a: u32, b: u32) -> Option<u32> {
        let fan = fans.plain(b)?;
        let ends: Vec<u32> = fan.iter().filter_map(|&(f, k)| self.faces.get(f).map(|face| face[(k + 1) % 4])).collect();
        let i = ends.iter().position(|&e| e == a)?;
        ends.get((i + 2) % 4).copied()
    }

    /// The fourth point of the face at `v` whose sides from `v` end at `a` and `b`.
    fn fourth(&self, fans: &Fans, v: u32, a: u32, b: u32) -> Option<u32> {
        fans.any(v)?.iter().find_map(|&(f, k)| {
            let face = self.faces.get(f)?;
            let (next, before) = (face[(k + 1) % 4], face[(k + 3) % 4]);
            ((next == a && before == b) || (next == b && before == a)).then_some(face[(k + 2) % 4])
        })
    }

    /// The sixteen cage points round the face `f`, as a grid: `[i][j]` is `i - 1` steps from the
    /// face's first corner toward its second and `j - 1` toward its fourth. None unless four
    /// faces meet at each of the face's corners.
    fn grid(&self, fans: &Fans, f: usize) -> Option<[[u32; 4]; 4]> {
        let [v0, v1, v2, v3] = *self.faces.get(f)?;
        let mut g = [[0_u32; 4]; 4];
        (g[1][1], g[2][1], g[2][2], g[1][2]) = (v0, v1, v2, v3);
        g[0][1] = self.straight(fans, v1, v0)?;
        g[3][1] = self.straight(fans, v0, v1)?;
        g[0][2] = self.straight(fans, v2, v3)?;
        g[3][2] = self.straight(fans, v3, v2)?;
        g[1][0] = self.straight(fans, v3, v0)?;
        g[1][3] = self.straight(fans, v0, v3)?;
        g[2][0] = self.straight(fans, v2, v1)?;
        g[2][3] = self.straight(fans, v1, v2)?;
        g[0][0] = self.fourth(fans, v0, g[0][1], g[1][0])?;
        g[3][0] = self.fourth(fans, v1, g[3][1], g[2][0])?;
        g[3][3] = self.fourth(fans, v2, g[3][2], g[2][3])?;
        g[0][3] = self.fourth(fans, v3, g[0][2], g[1][3])?;
        Some(g)
    }

    /// The two-ring of the irregular point `v`, numbered as the kernel's blends read it: `v`,
    /// then six points for each face of `fan` in turn. None unless the points one step from `v`
    /// are plain and every point two steps out is there.
    fn two_ring(&self, fans: &Fans, v: u32, fan: &[(usize, usize)]) -> Option<Vec<u32>> {
        let n = fan.len();
        let corner = |i: usize, step: usize| fan.get(i % n).and_then(|&(f, k)| self.faces.get(f).map(|face| face[(k + step) % 4]));
        let mut ring = vec![v];
        for i in 0..n {
            // This face's side along its first spoke, its far corner, and the next spoke's end.
            let (a10, a11, b10) = (corner(i, 1)?, corner(i, 2)?, corner(i + 1, 1)?);
            let a20 = self.straight(fans, v, a10)?;
            let b20 = self.straight(fans, v, b10)?;
            let a21 = self.fourth(fans, a10, a20, a11)?;
            let a12 = self.fourth(fans, b10, a11, b20)?;
            let a22 = self.fourth(fans, a11, a21, a12)?;
            ring.extend([a10, a11, a20, a21, a12, a22]);
        }
        Some(ring)
    }

    /// The cage's patches: see the module.
    pub fn patches(&self, bases: &mut CornerBases) -> Result<Patches, CageError> {
        let fans = Fans::of(self);
        let mut made: Vec<Option<CagePatch>> = vec![None; self.faces.len()];
        // The faces all of whose corners are plain.
        for (f, slot) in made.iter_mut().enumerate() {
            let Some(grid) = self.grid(&fans, f) else { continue };
            let mut blends = Vec::with_capacity(16);
            // The patch's point (a, b), a fastest: along u it takes row a of the span's Bézier
            // points, along v row b.
            for along_v in &TO_BEZIER {
                for along_u in &TO_BEZIER {
                    let mut blend: Vec<(u32, f64)> = Vec::with_capacity(9);
                    for (column, wu) in grid.iter().zip(along_u) {
                        for (&point, wv) in column.iter().zip(along_v) {
                            let w = wu * wv / 36.0;
                            if w != 0.0 {
                                add(&mut blend, point, w);
                            }
                        }
                    }
                    blends.push(blend);
                }
            }
            *slot = Some(CagePatch { face: f, degree: 3, blends });
        }
        // The faces round each irregular corner that stands apart.
        for v in 0..self.points.len() as u32 {
            let Some(fan) = fans.any(v) else { continue };
            let n = fan.len();
            if n == 4 || !(corner::MIN_FACES..=corner::MAX_FACES).contains(&n) {
                continue;
            }
            // Each face at the corner must have no other irregular corner, and the two-ring whole.
            let alone = fan.iter().all(|&(f, k)| self.faces.get(f).is_some_and(|face| (1..4).all(|s| fans.plain(face[(k + s) % 4]).is_some())));
            let Some(ring) = alone.then(|| self.two_ring(&fans, v, fan)).flatten() else { continue };
            let basis = bases.of(n)?;
            let side = corner::DEGREE + 1;
            for (i, &(f, k)) in fan.iter().enumerate() {
                let mut blends = Vec::with_capacity(corner::POINTS);
                for b in 0..side {
                    for a in 0..side {
                        // The kernel's patch has the corner at (0, 0) and u along the face's side
                        // from it; the face may have the corner as any of its four.
                        let (pa, pb) = match k {
                            0 => (a, b),
                            1 => (b, corner::DEGREE - a),
                            2 => (corner::DEGREE - a, corner::DEGREE - b),
                            _ => (corner::DEGREE - b, a),
                        };
                        let weights = corner::point_blend(basis, i, pa + side * pb).map_err(|e| CageError::Corner(e.to_string()))?;
                        let mut blend: Vec<(u32, f64)> = Vec::with_capacity(ring.len());
                        for (&point, &w) in ring.iter().zip(weights) {
                            if w != 0.0 {
                                add(&mut blend, point, w);
                            }
                        }
                        blends.push(blend);
                    }
                }
                if let Some(slot) = made.get_mut(f) {
                    *slot = Some(CagePatch { face: f, degree: corner::DEGREE, blends });
                }
            }
        }
        let left_out = made.iter().enumerate().filter(|(_, p)| p.is_none()).map(|(f, _)| f).collect();
        Ok(Patches { patches: made.into_iter().flatten().collect(), left_out })
    }
}

/// `w` more of `point` in `blend`: a cage point met twice round a small cage is one unknown.
fn add(blend: &mut Vec<(u32, f64)>, point: u32, w: f64) {
    match blend.iter_mut().find(|(p, _)| *p == point) {
        Some((_, sum)) => *sum += w,
        None => blend.push((point, w)),
    }
}

/// The faces round each point of a cage, counter-clockwise, where they go all the way round.
struct Fans(Vec<Option<Vec<(usize, usize)>>>);

impl Fans {
    fn of(cage: &Cage) -> Fans {
        let mut first: Vec<Option<(usize, usize)>> = vec![None; cage.points.len()];
        for (f, face) in cage.faces.iter().enumerate() {
            for (k, &v) in face.iter().enumerate() {
                if let Some(slot) = first.get_mut(v as usize) {
                    slot.get_or_insert((f, k));
                }
            }
        }
        Fans(first.iter().enumerate().map(|(v, start)| start.and_then(|s| cage.fan(v as u32, s))).collect())
    }

    /// The fan round `v`, when the faces go all the way round it.
    fn any(&self, v: u32) -> Option<&Vec<(usize, usize)>> {
        self.0.get(v as usize)?.as_ref()
    }

    /// The fan round `v`, when it is a plain point: four faces, all the way round.
    fn plain(&self, v: u32) -> Option<&Vec<(usize, usize)>> {
        self.any(v).filter(|fan| fan.len() == 4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type V = [f64; 3];

    fn sub(a: V, b: V) -> V {
        [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
    }

    fn dot(a: V, b: V) -> f64 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    fn cross(a: V, b: V) -> V {
        [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
    }

    fn length(a: V) -> f64 {
        dot(a, a).sqrt()
    }

    /// The Bernstein polynomials of degree `n` at `t`, or their `r`-th derivatives.
    fn bernstein(n: usize, t: f64, r: usize) -> Vec<f64> {
        let mut b = vec![1.0];
        for _ in 0..n - r {
            b = (0..=b.len()).map(|i| (if i > 0 { b[i - 1] * t } else { 0.0 }) + (if i < b.len() { b[i] * (1.0 - t) } else { 0.0 })).collect();
        }
        for d in n - r + 1..=n {
            b = (0..=b.len()).map(|i| d as f64 * ((if i > 0 { b[i - 1] } else { 0.0 }) - (if i < b.len() { b[i] } else { 0.0 }))).collect();
        }
        b
    }

    /// A patch's point or derivative at (u, v).
    fn at(points: &[Vec3], degree: usize, (u, v): (f64, f64), (ru, rv): (usize, usize)) -> V {
        let (bu, bv) = (bernstein(degree, u, ru), bernstein(degree, v, rv));
        let mut s = [0.0; 3];
        for b in 0..=degree {
            for a in 0..=degree {
                let p = points[a + (degree + 1) * b];
                let w = bu[a] * bv[b];
                s = [s[0] + w * p.x, s[1] + w * p.y, s[2] + w * p.z];
            }
        }
        s
    }

    /// Where a patch is, which way it faces and how it curves at (u, v): its point, its unit
    /// normal and its second fundamental form as a tensor in space, which two surfaces that meet
    /// in curvature share whatever their parameters.
    fn shape(points: &[Vec3], degree: usize, uv: (f64, f64)) -> (V, V, [[f64; 3]; 3]) {
        let d = |r| at(points, degree, uv, r);
        let (su, sv, suu, suv, svv) = (d((1, 0)), d((0, 1)), d((2, 0)), d((1, 1)), d((0, 2)));
        let normal = cross(su, sv);
        let size = length(normal);
        let normal = [normal[0] / size, normal[1] / size, normal[2] / size];
        let (e, f, g) = (dot(su, su), dot(su, sv), dot(sv, sv));
        let det = e * g - f * f;
        let inverse = [[g / det, -f / det], [-f / det, e / det]];
        let second = [[dot(suu, normal), dot(suv, normal)], [dot(suv, normal), dot(svv, normal)]];
        let frame = [su, sv];
        let mut tensor = [[0.0; 3]; 3];
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    for d in 0..2 {
                        let w = inverse[a][b] * second[b][c] * inverse[c][d];
                        for r in 0..3 {
                            for s in 0..3 {
                                tensor[r][s] += w * frame[a][r] * frame[d][s];
                            }
                        }
                    }
                }
            }
        }
        (d((0, 0)), normal, tensor)
    }

    /// The place a share `t` of the way along side `side` of a patch's square, the side running
    /// counter-clockwise from corner `side`.
    fn on_side(side: usize, t: f64) -> (f64, f64) {
        match side {
            0 => (t, 0.0),
            1 => (1.0, t),
            2 => (1.0 - t, 1.0),
            _ => (0.0, 1.0 - t),
        }
    }

    /// The most two patches differ along any edge two faces with patches share: in place, in
    /// the way they face and in how they curve; and how many such edges there are.
    fn worst_join(cage: &Cage, patches: &Patches) -> (f64, f64, f64, usize) {
        let of: HashMap<usize, (usize, Vec<Vec3>)> = patches.patches.iter().map(|p| (p.face, (p.degree, p.points(cage.points())))).collect();
        let mut worst = (0.0_f64, 0.0_f64, 0.0_f64, 0);
        for (&(from, to), &(f, k)) in &cage.along {
            let (Some(&(g, l)), Some(mine)) = (cage.along.get(&(to, from)), of.get(&f)) else { continue };
            let Some(theirs) = of.get(&g) else { continue };
            worst.3 += 1;
            for t in [0.05, 0.3, 0.5, 0.8, 0.95] {
                let a = shape(&mine.1, mine.0, on_side(k, t));
                let b = shape(&theirs.1, theirs.0, on_side(l, 1.0 - t));
                worst.0 = worst.0.max(length(sub(a.0, b.0)));
                worst.1 = worst.1.max(length(sub(a.1, b.1)));
                for r in 0..3 {
                    for s in 0..3 {
                        worst.2 = worst.2.max((a.2[r][s] - b.2[r][s]).abs());
                    }
                }
            }
        }
        worst
    }

    /// A cage round a torus: `around` faces the long way and `across` the short way, every point
    /// plain.
    fn torus(around: u32, across: u32) -> Cage {
        let mut points = Vec::new();
        for i in 0..around {
            for j in 0..across {
                let (a, b) = (std::f64::consts::TAU * f64::from(i) / f64::from(around), std::f64::consts::TAU * f64::from(j) / f64::from(across));
                points.push(Vec3::new((3.0 + b.cos()) * a.cos(), (3.0 + b.cos()) * a.sin(), b.sin()));
            }
        }
        let id = |i: u32, j: u32| (i % around) * across + j % across;
        let faces = (0..around).flat_map(|i| (0..across).map(move |j| [id(i, j), id(i + 1, j), id(i + 1, j + 1), id(i, j + 1)])).collect();
        Cage::new(points, faces).unwrap()
    }

    /// A cage round a ball: a cube of `k` × `k` faces a side, its points pushed out onto the
    /// unit sphere. Three faces meet at each of the cube's eight corners.
    fn ball(k: u32) -> Cage {
        let mut ids: HashMap<[i64; 3], u32> = HashMap::new();
        let mut points = Vec::new();
        let mut faces = Vec::new();
        // Each side: where it is, and its two directions, so that the faces look outward.
        let sides: [([f64; 3], [f64; 3], [f64; 3]); 6] = [
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        for (out, du, dv) in sides {
            let mut point = |i: u32, j: u32| {
                let (s, t) = (2.0 * f64::from(i) / f64::from(k) - 1.0, 2.0 * f64::from(j) / f64::from(k) - 1.0);
                let p = [out[0] + s * du[0] + t * dv[0], out[1] + s * du[1] + t * dv[1], out[2] + s * du[2] + t * dv[2]];
                let key = [(p[0] * 1e6).round() as i64, (p[1] * 1e6).round() as i64, (p[2] * 1e6).round() as i64];
                *ids.entry(key).or_insert_with(|| {
                    let size = length(p);
                    points.push(Vec3::new(p[0] / size, p[1] / size, p[2] / size));
                    points.len() as u32 - 1
                })
            };
            for i in 0..k {
                for j in 0..k {
                    faces.push([point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1)]);
                }
            }
        }
        Cage::new(points, faces).unwrap()
    }

    /// Where four faces meet everywhere, every face is a bicubic patch and all of them meet in
    /// curvature: one B-spline surface.
    #[test]
    fn a_torus_cage_is_all_bicubic_and_meets_in_curvature_everywhere() {
        let cage = torus(8, 6);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        assert!(patches.left_out.is_empty() && patches.patches.len() == 48);
        assert!(patches.patches.iter().all(|p| p.degree == 3 && p.blends.len() == 16));
        let (gap, facing, curving, edges) = worst_join(&cage, &patches);
        assert_eq!(edges, 2 * 96, "each of the 96 edges, from either side");
        assert!(gap < 1e-12 && facing < 1e-12 && curving < 1e-10, "{gap:e} apart, facing {facing:e} apart, curving {curving:e} apart");
    }

    /// A ball's cage has eight corners where three faces meet. Every face still gets a patch,
    /// the three at each corner of degree 7, and every edge of the cage meets in curvature: the
    /// bicubic faces with one another, the corner's faces with one another, and the two kinds
    /// with each other.
    #[test]
    fn a_ball_cage_meets_in_curvature_at_its_three_faced_corners_too() {
        let cage = ball(3);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        assert!(patches.left_out.is_empty(), "{:?}", patches.left_out);
        assert_eq!(patches.patches.iter().filter(|p| p.degree == 7).count(), 8 * 3);
        assert_eq!(patches.patches.iter().filter(|p| p.degree == 3).count(), 54 - 24);
        let (gap, facing, curving, edges) = worst_join(&cage, &patches);
        assert_eq!(edges, 2 * 108, "each of the 108 edges, from either side");
        assert!(gap < 1e-9 && facing < 1e-9 && curving < 1e-7, "{gap:e} apart, facing {facing:e} apart, curving {curving:e} apart");
        // Every weight set sums to one, so the patches move with the cage.
        for p in &patches.patches {
            for blend in &p.blends {
                let sum: f64 = blend.iter().map(|(_, w)| w).sum();
                assert!((sum - 1.0).abs() < 1e-9, "face {}: {sum}", p.face);
            }
        }
        // Each patch is a surface the rest of the app can take, and lies near the ball.
        for p in &patches.patches {
            let surface = p.surface(cage.points()).unwrap();
            let middle = surface.eval(0.5, 0.5).unwrap();
            let far = (middle.x * middle.x + middle.y * middle.y + middle.z * middle.z).sqrt();
            assert!((0.5..1.0).contains(&far), "face {}: {far}", p.face);
        }
    }

    /// With two faces a side every face of the ball has one of the cube's corners, and the
    /// corners are two steps apart: no face has two, and no two share an edge, which is all the
    /// construction asks. All 24 faces are corner patches, and they meet one another in
    /// curvature across every edge, the edges between two corners' faces too.
    #[test]
    fn corners_two_steps_apart_still_meet_in_curvature() {
        let cage = ball(2);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        assert!(patches.left_out.is_empty() && patches.patches.iter().all(|p| p.degree == 7));
        let (gap, facing, curving, edges) = worst_join(&cage, &patches);
        assert_eq!(edges, 2 * 48);
        assert!(gap < 1e-9 && facing < 1e-9 && curving < 1e-7, "{gap:e} apart, facing {facing:e} apart, curving {curving:e} apart");
    }

    /// A plain cube: every face has four irregular corners, so none gets a patch, and nothing
    /// is made up.
    #[test]
    fn a_face_with_more_than_one_irregular_corner_is_left_out() {
        let cage = ball(1);
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        assert!(patches.patches.is_empty() && patches.left_out.len() == 6);
    }

    /// An open sheet: only the faces a step in from the rim all round have patches.
    #[test]
    fn faces_at_the_rim_of_an_open_cage_are_left_out() {
        let n = 6_u32;
        let points = (0..n * n).map(|k| Vec3::new(f64::from(k % n), f64::from(k / n), 0.1 * f64::from(k % n) * f64::from(k / n))).collect();
        let faces = (0..n - 1).flat_map(|j| (0..n - 1).map(move |i| [j * n + i, j * n + i + 1, (j + 1) * n + i + 1, (j + 1) * n + i])).collect();
        let cage = Cage::new(points, faces).unwrap();
        let patches = cage.patches(&mut CornerBases::default()).unwrap();
        assert_eq!(patches.patches.len(), 9, "the 3 × 3 faces in the middle of 5 × 5");
        assert_eq!(patches.left_out.len(), 16);
        let (gap, facing, curving, _) = worst_join(&cage, &patches);
        assert!(gap < 1e-12 && facing < 1e-12 && curving < 1e-10);
    }

    #[test]
    fn what_is_no_cage_is_refused() {
        let square = vec![Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
        assert_eq!(Cage::new(square.clone(), vec![[0, 1, 2, 4]]).unwrap_err(), CageError::BadFace(0));
        assert_eq!(Cage::new(square.clone(), vec![[0, 1, 1, 3]]).unwrap_err(), CageError::BadFace(0));
        assert_eq!(Cage::new(square.clone(), vec![[0, 1, 2, 3], [0, 1, 2, 3]]).unwrap_err(), CageError::Folded(1));
        let mut bad = square;
        bad[2].x = f64::NAN;
        assert_eq!(Cage::new(bad, vec![[0, 1, 2, 3]]).unwrap_err(), CageError::BadPoint(2));
    }
}
