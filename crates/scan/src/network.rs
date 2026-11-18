//! A Free Form network (QuickSurface's quad surface, by way of SurfaceCraft's cage): faces, each a
//! cubic B-spline net, that share whole sides. A point on a shared side is one point of the
//! network, held by both nets, so the faces meet exactly; where two faces met smoothly when the
//! network was read (most of the seam's points meet straight), the points next to their shared
//! side are kept in line with it either side, so the faces stay tangent there as they are edited;
//! where they met sharply (a crease) only the side is held.
//!
//! The network is read from its faces alone ([`joins`]): points of different faces that are in
//! the same place are one point. So a network is kept as its faces (a sewn surface body), a
//! corner dropped on another is welded by being put in its place ([`welded`]), and a design keeps
//! only the seams creased or held smooth by hand where the faces' angle reads them otherwise
//! ([`held_by_hand`], read back with [`joins_keeping`]).
//!
//! An edit is least squares over the points that may move ([`moved`]): places on the faces put
//! where they go (a point dragged, the points round it held where they are, scan places to fit),
//! the faces held tangent across smooth seams, and a light hold on how unevenly neighbouring
//! points move. The points a user drags are on the surface ([`dots`]: the surface at each point's
//! Greville place), as QuickSurface's feature points are; the control net is the cage behind them.

use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;

use crate::merge::SIDES;
pub use crate::merge::Side;
use crate::surface::{cholesky_back, cholesky_factor, clamped_knots};
use crate::surface_edit::{basis, foot_from, frame, greville_places, points, samples, with_points};

/// How far from straight across a seam two faces may meet and still be held tangent, degrees.
pub const SMOOTH_DEGREES: f64 = 3.0;

/// The most faces a network has.
pub const MOST_FACES: usize = 400;

/// Points closer than this share of the network's size are one point.
const SAME_POINT: f64 = 1e-7;

/// The most free points solved for with a dense factoring; more go by conjugate gradients.
const DENSE: usize = 900;

/// Two faces' sides that are one: the same network points, in order, or the other way round.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seam {
    pub a: usize,
    pub a_side: Side,
    pub b: usize,
    pub b_side: Side,
    pub reversed: bool,
}

/// A corner of the network's faces: the point, how many faces have a corner there, and whether it
/// lies on the network's edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Corner {
    pub point: usize,
    pub faces: usize,
    pub border: bool,
}

impl Corner {
    /// Not as a grid's corner is: inside, other than four faces round it; on the edge, more than
    /// three (three are a grid's corner with a quarter left out). The surface cannot be smooth all
    /// round such a point (QuickSurface's red points).
    pub fn irregular(&self) -> bool {
        if self.border { self.faces > 3 } else { self.faces != 4 }
    }
}

/// A point on a seam (`e`) and the points next to it either side, `a` in one face and `b` in the
/// other: held in a line, each seam's `b` as far from `e` as its `a` is, times one ratio for the
/// whole seam, so the faces are tangent all along it and not only at their points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Across {
    pub seam: usize,
    pub e: usize,
    pub a: usize,
    pub b: usize,
}

/// How the faces of a network are joined.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Joins {
    /// For each face, the network point each point of its net is (u fastest).
    pub of: Vec<Vec<usize>>,
    /// Each network point's places: a face and a point of its net, the first its own.
    pub at: Vec<Vec<(usize, usize)>>,
    /// The sides two faces share.
    pub seams: Vec<Seam>,
    /// The sides of one face only: the network's edge.
    pub open: Vec<(usize, Side)>,
    /// Across each smooth seam: a point on it and the points next to it inside the two faces,
    /// held in a line.
    pub smooth: Vec<Across>,
    pub corners: Vec<Corner>,
}

impl Joins {
    /// How many points the network has.
    pub fn len(&self) -> usize {
        self.at.len()
    }

    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }

    /// Whether network point `p` is on a side two faces share.
    pub fn on_seam(&self, p: usize) -> bool {
        self.at.get(p).is_some_and(|at| at.len() > 1)
    }

    /// The network points along `side` of `face`, in its order.
    pub fn side(&self, faces: &[NurbsSurface], face: usize, side: Side) -> Vec<usize> {
        let (Some(f), Some(of)) = (faces.get(face), self.of.get(face)) else { return Vec::new() };
        side.indices(f.count()).iter().filter_map(|k| of.get(*k).copied()).collect()
    }

    pub fn irregular(&self) -> impl Iterator<Item = &Corner> {
        self.corners.iter().filter(|c| c.irregular())
    }
}

/// Why a network could not be read or changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkError {
    /// A face is not a cubic B-spline without weights.
    NotCubic,
    TooManyFaces,
    /// The side asked for is not on the network's edge.
    NotOpen,
    /// The side is too short to build on.
    TooShort,
    /// The face's places are not finite, or no face could be fitted.
    Failed,
    /// A face would have a corner as flat as a straight line (its two sides there running on from
    /// each other): no four-sided face can be made so.
    FlatCorner,
    /// A Free Form keeps one face at least.
    LastFace,
    /// Taking the face away would leave the faces in two pieces.
    WouldSplit,
    /// The faces close on themselves: thickened they would be two shells, not one solid.
    Closed,
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NetworkError::NotCubic => "only cubic surfaces without weights (as New Free Form and Fit Surface make them) join a network",
            NetworkError::TooManyFaces => "a Free Form has at most 400 faces",
            NetworkError::NotOpen => "a face is added on an open edge: that edge has a face on it already",
            NetworkError::TooShort => "that edge has too few points to build on",
            NetworkError::Failed => "the face could not be made there: click its far corners away from the edge, on the scan",
            NetworkError::FlatCorner => "that face would have a corner as flat as a straight line, where two sides it shares run on from each other (a single face over a smooth rim): close the opening with several faces, a middle one and one on each side, so that every face meets the rim at a corner",
            NetworkError::LastFace => "a Free Form keeps one face at least: delete the surface body to take it all away",
            NetworkError::WouldSplit => "taking that face away would leave the Free Form in two pieces: take away a face on its edge",
            NetworkError::Closed => "the Free Form closes on itself, so thickened it would be two shells: hollow the solid it makes with Shell instead",
        })
    }
}

fn cubic(s: &NurbsSurface) -> bool {
    s.degree() == [3, 3] && s.count().iter().all(|n| *n >= 4) && s.homogeneous().iter().all(|h| (h.w - 1.0).abs() <= 1e-12)
}

/// One step into the face from a point of `side` (at `k` in the net of `count`).
fn inward(side: Side, [nu, _]: [usize; 2], k: usize) -> usize {
    match side {
        Side::FirstRow => k + nu,
        Side::LastRow => k - nu,
        Side::FirstColumn => k + 1,
        Side::LastColumn => k - 1,
    }
}

/// The four corners of a net of `count`, as places in it.
fn corner_places([nu, nv]: [usize; 2]) -> [usize; 4] {
    [0, nu - 1, nu * nv - 1, (nv - 1) * nu]
}

fn find(parent: &mut [usize], mut a: usize) -> usize {
    while parent[a] != a {
        parent[a] = parent[parent[a]];
        a = parent[a];
    }
    a
}

/// How the faces join: points of different faces' sides within a ten-millionth of the network's
/// size of each other are one point; sides all of whose points are one are seams.
pub fn joins(faces: &[NurbsSurface]) -> Result<Joins, NetworkError> {
    if faces.len() > MOST_FACES {
        return Err(NetworkError::TooManyFaces);
    }
    if faces.iter().any(|f| !cubic(f)) {
        return Err(NetworkError::NotCubic);
    }
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    if nets.iter().flatten().any(|p| !p.is_finite()) {
        return Err(NetworkError::Failed);
    }
    let starts: Vec<usize> = nets.iter().scan(0, |s, n| Some(std::mem::replace(s, *s + n.len()))).collect();
    let total = nets.iter().map(Vec::len).sum::<usize>();
    let (lo, hi) = nets
        .iter()
        .flatten()
        .fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let tolerance = (SAME_POINT * hi.dist(lo)).max(1e-12);
    // The points on the faces' sides, swept along x: those within the tolerance are one.
    let mut border: Vec<(usize, Vec3)> = Vec::new();
    for (f, face) in faces.iter().enumerate() {
        let mut on: Vec<usize> = SIDES.iter().flat_map(|s| s.indices(face.count())).collect();
        on.sort_unstable();
        on.dedup();
        border.extend(on.into_iter().map(|k| (starts[f] + k, nets[f][k])));
    }
    border.sort_by(|a, b| a.1.x.total_cmp(&b.1.x));
    let mut parent: Vec<usize> = (0..total).collect();
    for (n, &(a, p)) in border.iter().enumerate() {
        for &(b, q) in border[n + 1..].iter().take_while(|(_, q)| q.x - p.x <= tolerance) {
            if p.dist(q) <= tolerance {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra.max(rb)] = ra.min(rb);
                }
            }
        }
    }
    let mut id_of_root: HashMap<usize, usize> = HashMap::new();
    let mut at: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut of: Vec<Vec<usize>> = Vec::with_capacity(faces.len());
    for (f, net) in nets.iter().enumerate() {
        let mut ids = Vec::with_capacity(net.len());
        for k in 0..net.len() {
            let root = find(&mut parent, starts[f] + k);
            let id = *id_of_root.entry(root).or_insert_with(|| {
                at.push(Vec::new());
                at.len() - 1
            });
            at[id].push((f, k));
            ids.push(id);
        }
        of.push(ids);
    }
    // Sides by their ends: two sides of the same points, either way round, are a seam.
    let side_ids = |f: usize, s: Side| -> Vec<usize> { s.indices(faces[f].count()).iter().map(|k| of[f][*k]).collect() };
    let mut by_ends: HashMap<(usize, usize, usize), Vec<(usize, Side)>> = HashMap::new();
    for f in 0..faces.len() {
        for s in SIDES {
            let ids = side_ids(f, s);
            let (Some(&first), Some(&last)) = (ids.first(), ids.last()) else { continue };
            if first == last {
                continue;
            }
            by_ends.entry((first.min(last), first.max(last), ids.len())).or_default().push((f, s));
        }
    }
    let mut seams = Vec::new();
    let mut paired: Vec<(usize, Side)> = Vec::new();
    let mut keys: Vec<_> = by_ends.keys().copied().collect();
    keys.sort_unstable();
    for key in keys {
        let sides = &by_ends[&key];
        for (n, &(a, sa)) in sides.iter().enumerate() {
            for &(b, sb) in &sides[n + 1..] {
                if paired.contains(&(a, sa)) || paired.contains(&(b, sb)) || (a == b && sa == sb) {
                    continue;
                }
                let (ia, ib) = (side_ids(a, sa), side_ids(b, sb));
                let reversed = ia.first() != ib.first();
                let same = if reversed { ia.iter().eq(ib.iter().rev()) } else { ia == ib };
                if same {
                    seams.push(Seam { a, a_side: sa, b, b_side: sb, reversed });
                    paired.push((a, sa));
                    paired.push((b, sb));
                }
            }
        }
    }
    let open: Vec<(usize, Side)> = (0..faces.len()).flat_map(|f| SIDES.map(|s| (f, s))).filter(|fs| !paired.contains(fs)).collect();
    // A corner's faces, and whether it is on an open side.
    let on_open: std::collections::HashSet<usize> = open.iter().flat_map(|&(f, s)| side_ids(f, s)).collect();
    let mut corner_faces: HashMap<usize, Vec<usize>> = HashMap::new();
    for (f, face) in faces.iter().enumerate() {
        for k in corner_places(face.count()) {
            corner_faces.entry(of[f][k]).or_default().push(f);
        }
    }
    let mut corners: Vec<Corner> =
        corner_faces.into_iter().map(|(point, fs)| Corner { point, faces: fs.len(), border: on_open.contains(&point) }).collect();
    corners.sort_by_key(|c| c.point);
    let irregular: std::collections::HashSet<usize> = corners.iter().filter(|c| c.irregular()).map(|c| c.point).collect();
    let border_corners: std::collections::HashSet<usize> = corners.iter().filter(|c| c.border).map(|c| c.point).collect();
    // Tangency held across seams that are smooth: along them, and at their ends where the corner
    // is as a grid's (an irregular corner cannot be held so: its faces would pull against each
    // other; one on the network's edge only where its open sides run on: see [`end_held`]). A
    // seam is one thing: smooth when most of its points meet straight (the few that do not are
    // held straight too), a crease when most meet at an angle (none is held: a crease's ends, on
    // the network's edge, often still meet smoothly, and must not round it over again).
    let mut smooth = Vec::new();
    for (index, seam) in seams.iter().enumerate() {
        let (ca, cb) = (faces[seam.a].count(), faces[seam.b].count());
        let (ka, mut kb) = (seam.a_side.indices(ca), seam.b_side.indices(cb));
        if seam.reversed {
            kb.reverse();
        }
        let mut held = Vec::new();
        let (mut straight_points, mut sharp_points) = (0usize, 0usize);
        for (n, (&pa, &pb)) in ka.iter().zip(&kb).enumerate() {
            let e = of[seam.a][pa];
            let (ia, ib) = (of[seam.a][inward(seam.a_side, ca, pa)], of[seam.b][inward(seam.b_side, cb, pb)]);
            if ia == ib {
                continue;
            }
            let (pe, pia, pib) = (nets[seam.a][pa], nets[seam.a][inward(seam.a_side, ca, pa)], nets[seam.b][inward(seam.b_side, cb, pb)]);
            if (n == 0 || n + 1 == ka.len()) && !end_held(irregular.contains(&e), border_corners.contains(&e), pia, pe, pib) {
                continue;
            }
            let (da, db) = (pia - pe, pe - pib);
            let straight = match (da.normalized(), db.normalized()) {
                (Some(a), Some(b)) => a.dot(b) >= SMOOTH_DEGREES.to_radians().cos(),
                _ => false,
            };
            if straight {
                straight_points += 1
            } else {
                sharp_points += 1
            }
            held.push(Across { seam: index, e, a: ia, b: ib });
        }
        if sharp_points <= straight_points {
            smooth.extend(held);
        }
    }
    Ok(Joins { of, at, seams, open, smooth, corners })
}

/// The network's points, each once, read from the faces (a point's own place).
pub fn network_points(faces: &[NurbsSurface], joins: &Joins) -> Vec<Vec3> {
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    joins.at.iter().map(|at| at.first().and_then(|&(f, k)| nets.get(f)?.get(k).copied()).unwrap_or(Vec3::ZERO)).collect()
}

/// The faces with the network's points at `places` (one per network point).
pub fn with_network_points(faces: &[NurbsSurface], joins: &Joins, places: &[Vec3]) -> Option<Vec<NurbsSurface>> {
    faces
        .iter()
        .zip(&joins.of)
        .map(|(face, ids)| {
            let net: Option<Vec<Vec3>> = ids.iter().map(|id| places.get(*id).copied()).collect();
            with_points(face, &net?)
        })
        .collect()
}

/// Where on a face the points a user drags lie, one for each of its control points (u fastest):
/// evenly spaced along each way whose knots are (as a Free Form's are), so they do not crowd its
/// edges as the control points' Greville places do; else at the Greville places. Either way the
/// surface can be made to run through any places put there (each point's own basis is not zero
/// at its place).
pub fn dot_places(face: &NurbsSurface) -> Vec<(f64, f64)> {
    let even = |d: usize| -> bool {
        let (k, p) = (&face.knots()[d], face.degree()[d]);
        let inner: Vec<f64> = k.iter().copied().skip(p).take(k.len().saturating_sub(2 * p)).collect();
        inner.windows(3).all(|w| ((w[2] - w[1]) - (w[1] - w[0])).abs() <= 1e-9 * (k[k.len() - 1] - k[0]).abs().max(1e-300))
            && inner.windows(2).all(|w| w[1] > w[0])
    };
    let [nu, nv] = face.count();
    let [[u0, u1], [v0, v1]] = face.domain();
    let greville = greville_places(face);
    let (eu, ev) = (even(0), even(1));
    (0..nv)
        .flat_map(|j| (0..nu).map(move |i| (i, j)))
        .map(|(i, j)| {
            let (gu, gv) = greville[j * nu + i];
            let u = if eu { u0 + (u1 - u0) * i as f64 / (nu - 1).max(1) as f64 } else { gu };
            let v = if ev { v0 + (v1 - v0) * j as f64 / (nv - 1).max(1) as f64 } else { gv };
            (u, v)
        })
        .collect()
}

/// A point the user drags: on the surface, at a network point's own place's [`dot_places`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub face: usize,
    pub u: f64,
    pub v: f64,
    pub at: Vec3,
}

/// One dot per network point, in the network's order.
pub fn dots(faces: &[NurbsSurface], joins: &Joins) -> Vec<Dot> {
    let places: Vec<Vec<(f64, f64)>> = faces.iter().map(dot_places).collect();
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    joins
        .at
        .iter()
        .map(|at| {
            let &(face, k) = at.first().unwrap_or(&(0, 0));
            let (u, v) = places.get(face).and_then(|p| p.get(k)).copied().unwrap_or((0.0, 0.0));
            let at = faces.get(face).zip(nets.get(face)).and_then(|(f, n)| frame(f, n, u, v)).map_or(Vec3::ZERO, |(s, _, _)| s);
            Dot { face, u, v, at }
        })
        .collect()
}

/// The way the surface faces at a dot.
pub fn dot_normal(faces: &[NurbsSurface], dot: &Dot) -> Option<Vec3> {
    let face = faces.get(dot.face)?;
    let (_, su, sv) = frame(face, &points(face), dot.u, dot.v)?;
    su.cross(sv).normalized()
}

/// The open side whose ends are nearest `a` and `b` (either way round), and how far they are off
/// in all.
pub fn open_side_near(faces: &[NurbsSurface], joins: &Joins, a: Vec3, b: Vec3) -> Option<((usize, Side), f64)> {
    let places = network_points(faces, joins);
    joins
        .open
        .iter()
        .filter_map(|&(f, s)| {
            let ids = joins.side(faces, f, s);
            let (p, q) = (places[*ids.first()?], places[*ids.last()?]);
            Some(((f, s), (p.dist(a) + q.dist(b)).min(p.dist(b) + q.dist(a))))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1))
}

/// Whether every face is joined to every other by seams (one piece).
pub fn connected(faces: &[NurbsSurface], joins: &Joins) -> bool {
    if faces.len() <= 1 {
        return true;
    }
    let mut reached = vec![false; faces.len()];
    let mut stack = vec![0];
    reached[0] = true;
    while let Some(f) = stack.pop() {
        for seam in joins.seams.iter().filter(|s| s.a == f || s.b == f) {
            let other = if seam.a == f { seam.b } else { seam.a };
            if !reached[other] {
                reached[other] = true;
                stack.push(other);
            }
        }
    }
    reached.iter().all(|r| *r)
}

/// A place on a face put where it goes: the surface of `face` at `(u, v)` drawn toward `to`, as
/// hard as `weight` (a scan place counts 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Goal {
    pub face: usize,
    pub u: f64,
    pub v: f64,
    pub to: Vec3,
    pub weight: f64,
}

/// How the network moves in one step of [`moved`].
#[derive(Clone, Copy, Debug)]
pub struct Moves<'a> {
    /// The network points that may move.
    pub free: &'a [bool],
    pub goals: &'a [Goal],
    /// How hard the faces are held tangent across smooth seams, as so many places a point.
    pub tangent: f64,
    /// How evenly neighbouring points move (second differences of the move), as places a point.
    pub even: f64,
    /// How straight the net itself is held (second differences of its points), as places a
    /// point: for a new face, whose net starts as a guess, not for one being edited.
    pub bend: f64,
    /// Seams held tangent though they are not smooth yet (a face just added).
    pub also_smooth: &'a [Across],
}

/// A row of the least squares: network points, their shares, the place it wants and how hard.
struct Row {
    cols: Vec<(usize, f64)>,
    to: Vec3,
    weight: f64,
}

/// One least-squares step: the points `moves.free` marks moved to bring the goals nearest, the
/// faces tangent across smooth seams and neighbouring points moving evenly; the rest stay. `None`
/// when nothing may move or the solve has no answer.
pub fn moved(faces: &[NurbsSurface], joins: &Joins, moves: &Moves) -> Option<Vec<NurbsSurface>> {
    let start = network_points(faces, joins);
    let n = start.len();
    if moves.free.len() != n {
        return None;
    }
    let mut slot = vec![usize::MAX; n];
    let mut free = 0;
    for (p, f) in moves.free.iter().enumerate() {
        if *f {
            slot[p] = free;
            free += 1;
        }
    }
    if free == 0 {
        return None;
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut weights: Vec<f64> = Vec::new();
    for g in moves.goals {
        let (Some(face), Some(ids)) = (faces.get(g.face), joins.of.get(g.face)) else { continue };
        if !(g.weight > 0.0 && g.to.is_finite() && g.u.is_finite() && g.v.is_finite()) {
            continue;
        }
        let Some(b) = basis(face, g.u, g.v) else { continue };
        let mut cols: Vec<(usize, f64)> = Vec::with_capacity(16);
        for (&c, &w) in b.cols.iter().zip(&b.value) {
            let Some(&id) = ids.get(c) else { continue };
            match cols.iter_mut().find(|(i, _)| *i == id) {
                Some(e) => e.1 += w,
                None => cols.push((id, w)),
            }
        }
        if cols.iter().any(|(id, _)| moves.free[*id]) {
            weights.push(g.weight);
            rows.push(Row { cols, to: g.to, weight: g.weight });
        }
    }
    // What the goals weigh for each free point, by their middle weight: a point dragged weighs far
    // more than those held, and must not make the holds between points weigh more with it.
    weights.sort_by(f64::total_cmp);
    let middle = weights.get(weights.len() / 2).copied().unwrap_or(1.0);
    let per_point = (middle * weights.len() as f64 / free as f64).max(1.0);
    // Tangent across smooth seams: the inner point either side in line with the seam's, the far
    // one as far off as the near one times the seam's own ratio (of their lengths as they are, all
    // along it): with one ratio the faces' slopes across the seam agree everywhere along it, where
    // a ratio for each point would leave them kinked between the points.
    let across: Vec<Across> = joins.smooth.iter().chain(moves.also_smooth).copied().collect();
    let mut sums: HashMap<usize, (f64, f64)> = HashMap::new();
    for t in &across {
        let s = sums.entry(t.seam).or_insert((0.0, 0.0));
        s.0 += start[t.a].dist(start[t.e]);
        s.1 += start[t.b].dist(start[t.e]);
    }
    for t in &across {
        let (e, a, b) = (t.e, t.a, t.b);
        if !(moves.free[e] || moves.free[a] || moves.free[b]) {
            continue;
        }
        let (near, far) = sums.get(&t.seam).copied().unwrap_or((0.0, 0.0));
        if !(near > 1e-12 && far > 1e-12) {
            continue;
        }
        let r = near / far;
        rows.push(Row { cols: vec![(a, 1.0), (b, r), (e, -(1.0 + r))], to: Vec3::ZERO, weight: moves.tangent * per_point });
    }
    // At an irregular corner inside the network whose seams are all held smooth: the points next to
    // it along its seams in one plane with it, the plane the faces there face now (each row asks
    // for its point's way from the corner as it is, less its part off that plane). The rows across
    // the seams leave such a corner out, so without this the faces round it would meet at an angle
    // there and kink along each seam for a span from it.
    let held: std::collections::HashSet<usize> = across.iter().map(|t| t.seam).collect();
    for (corner, next, normal) in corner_planes(faces, joins, &start, &held) {
        for p in next {
            if !(moves.free[p] || moves.free[corner]) {
                continue;
            }
            let way = start[p] - start[corner];
            rows.push(Row { cols: vec![(p, 1.0), (corner, -1.0)], to: way - normal * way.dot(normal), weight: moves.tangent * per_point });
        }
    }
    // Even moves, and a straight net, along each face's rows and columns.
    for (face, ids) in faces.iter().zip(&joins.of) {
        let [nu, nv] = face.count();
        let mut lines: Vec<[usize; 3]> = Vec::new();
        for j in 0..nv {
            for i in 1..nu - 1 {
                lines.push([ids[j * nu + i - 1], ids[j * nu + i], ids[j * nu + i + 1]]);
            }
        }
        for i in 0..nu {
            for j in 1..nv - 1 {
                lines.push([ids[(j - 1) * nu + i], ids[j * nu + i], ids[(j + 1) * nu + i]]);
            }
        }
        for [a, b, c] in lines {
            if !(moves.free[a] || moves.free[b] || moves.free[c]) {
                continue;
            }
            let cols = vec![(a, 1.0), (b, -2.0), (c, 1.0)];
            let now = start[a] - start[b] * 2.0 + start[c];
            if moves.even > 0.0 {
                rows.push(Row { cols: cols.clone(), to: now, weight: moves.even * per_point });
            }
            if moves.bend > 0.0 {
                rows.push(Row { cols, to: Vec3::ZERO, weight: moves.bend * per_point });
            }
        }
    }
    // A light pull toward where each free point is, so one nothing reaches stays put.
    for (p, f) in moves.free.iter().enumerate() {
        if *f {
            rows.push(Row { cols: vec![(p, 1.0)], to: start[p], weight: 1e-6 * per_point });
        }
    }
    let solved = solve(&rows, &start, &slot, free)?;
    let mut places = start;
    for (p, s) in slot.iter().enumerate() {
        if let Some(q) = solved.get(*s) {
            places[p] = *q;
        }
    }
    with_network_points(faces, joins, &places)
}

/// Each irregular corner inside the network all of whose seams are in `held`: the corner, the
/// points next to it along its seams, and the plane its faces face there (the mean of their
/// normals at it, each turned to agree with the first's), `places` the network's points.
fn corner_planes(faces: &[NurbsSurface], joins: &Joins, places: &[Vec3], held: &std::collections::HashSet<usize>) -> Vec<(usize, Vec<usize>, Vec3)> {
    let mut out = Vec::new();
    for c in joins.corners.iter().filter(|c| c.irregular() && !c.border) {
        let mut next = Vec::new();
        let mut all_held = true;
        for (index, seam) in joins.seams.iter().enumerate() {
            let ids = joins.side(faces, seam.a, seam.a_side);
            let (Some(&first), Some(&last)) = (ids.first(), ids.last()) else { continue };
            let n = if first == c.point {
                ids.get(1)
            } else if last == c.point {
                ids.len().checked_sub(2).and_then(|k| ids.get(k))
            } else {
                continue;
            };
            all_held &= held.contains(&index);
            next.extend(n.copied());
        }
        if !all_held || next.len() < 3 {
            continue;
        }
        let mut sum = Vec3::ZERO;
        let mut first: Option<Vec3> = None;
        for &(f, k) in joins.at.get(c.point).map(Vec::as_slice).unwrap_or(&[]) {
            let Some(face) = faces.get(f) else { continue };
            let [nu, nv] = face.count();
            let [[u0, u1], [v0, v1]] = face.domain();
            let (u, v) = if k == 0 {
                (u0, v0)
            } else if k == nu - 1 {
                (u1, v0)
            } else if k == nu * nv - 1 {
                (u1, v1)
            } else {
                (u0, v1)
            };
            let Some((_, su, sv)) = frame(face, &points(face), u, v) else { continue };
            let Some(n) = su.cross(sv).normalized() else { continue };
            let n = match first {
                Some(f) if f.dot(n) < 0.0 => -n,
                Some(_) => n,
                None => {
                    first = Some(n);
                    n
                }
            };
            sum += n;
        }
        if let Some(normal) = sum.normalized()
            && places.get(c.point).is_some()
        {
            next.sort_unstable();
            next.dedup();
            out.push((c.point, next, normal));
        }
    }
    out
}

/// The free points (`slot` numbers them) that bring the rows nearest, the others where `at` has them.
fn solve(rows: &[Row], at: &[Vec3], slot: &[usize], free: usize) -> Option<Vec<Vec3>> {
    // The normal equations, by their entries; three right-hand sides.
    let mut entries: HashMap<(usize, usize), f64> = HashMap::new();
    let mut rhs = vec![Vec3::ZERO; free];
    for row in rows {
        let w = row.weight;
        let rest = row.cols.iter().filter(|(p, _)| slot[*p] == usize::MAX).fold(row.to, |r, (p, c)| r - at[*p] * *c);
        for &(pa, ca) in row.cols.iter().filter(|(p, _)| slot[*p] != usize::MAX) {
            let sa = slot[pa];
            rhs[sa] += rest * (w * ca);
            for &(pb, cb) in row.cols.iter().filter(|(p, _)| slot[*p] != usize::MAX) {
                *entries.entry((sa, slot[pb])).or_insert(0.0) += w * ca * cb;
            }
        }
    }
    let column = |c: usize| rhs.iter().map(|r| [r.x, r.y, r.z][c]).collect::<Vec<f64>>();
    let (x, y, z) = if free <= DENSE {
        let mut m = vec![0.0; free * free];
        for (&(a, b), v) in &entries {
            m[a * free + b] += v;
        }
        if !cholesky_factor(&mut m, free) {
            return None;
        }
        (cholesky_back(&m, &column(0), free)?, cholesky_back(&m, &column(1), free)?, cholesky_back(&m, &column(2), free)?)
    } else {
        let mut by_row: Vec<Vec<(usize, f64)>> = vec![Vec::new(); free];
        for (&(a, b), v) in &entries {
            by_row[a].push((b, *v));
        }
        let start = |c: usize| (0..slot.len()).filter(|p| slot[*p] != usize::MAX).map(|p| [at[p].x, at[p].y, at[p].z][c]).collect::<Vec<f64>>();
        let order: Vec<usize> = (0..slot.len()).filter(|p| slot[*p] != usize::MAX).collect();
        let guess = |c: usize| {
            let mut x = vec![0.0; free];
            for (k, p) in order.iter().enumerate() {
                x[slot[*p]] = start(c)[k];
            }
            conjugate_gradients(&by_row, &column(c), x)
        };
        (guess(0)?, guess(1)?, guess(2)?)
    };
    Some((0..free).map(|i| Vec3::new(x[i], y[i], z[i])).collect())
}

/// `M x = b` for the sparse symmetric positive definite `M` (its rows), by conjugate gradients
/// with the diagonal as preconditioner, from `x`.
fn conjugate_gradients(m: &[Vec<(usize, f64)>], b: &[f64], mut x: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    let apply = |v: &[f64], out: &mut [f64]| {
        for (i, row) in m.iter().enumerate() {
            out[i] = row.iter().map(|(j, a)| a * v[*j]).sum();
        }
    };
    let diagonal: Vec<f64> = m.iter().enumerate().map(|(i, row)| row.iter().filter(|(j, _)| *j == i).map(|(_, a)| *a).sum()).collect();
    let pre = |i: usize, v: f64| if diagonal[i] > 1e-300 { v / diagonal[i] } else { v };
    let mut ax = vec![0.0; n];
    apply(&x, &mut ax);
    let mut r: Vec<f64> = (0..n).map(|i| b[i] - ax[i]).collect();
    let mut z: Vec<f64> = (0..n).map(|i| pre(i, r[i])).collect();
    let mut p = z.clone();
    let mut rz: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
    let norm = b.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-300);
    let mut ap = vec![0.0; n];
    for _ in 0..(4 * n).max(200) {
        if r.iter().map(|v| v * v).sum::<f64>().sqrt() <= 1e-10 * norm {
            break;
        }
        apply(&p, &mut ap);
        let pap: f64 = p.iter().zip(&ap).map(|(a, b)| a * b).sum();
        if !(pap > 0.0) {
            break;
        }
        let alpha = rz / pap;
        for i in 0..n {
            x[i] += alpha * p[i];
            r[i] -= alpha * ap[i];
        }
        z = (0..n).map(|i| pre(i, r[i])).collect();
        let next: f64 = r.iter().zip(&z).map(|(a, b)| a * b).sum();
        let beta = next / rz;
        rz = next;
        for i in 0..n {
            p[i] = z[i] + beta * p[i];
        }
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// The scan points' places on the faces: for each point, the face whose surface it lies nearest
/// (its foot inside the face, within `reach`), as a goal of weight 1 there; points on no face (off
/// every edge, or farther than `reach`) left out. Only the faces `among` marks are looked at.
pub fn feet(faces: &[NurbsSurface], points_of_scan: &[Vec3], reach: f64, among: &[bool]) -> Vec<Goal> {
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    let guesses: Vec<Vec<(f64, f64, Vec3)>> = faces.iter().zip(&nets).map(|(f, n)| samples(f, n)).collect();
    let boxes: Vec<(Vec3, Vec3)> = nets
        .iter()
        .map(|n| {
            n.iter().fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
        })
        .map(|(lo, hi)| (lo - Vec3::new(reach, reach, reach), hi + Vec3::new(reach, reach, reach)))
        .collect();
    let mut goals = Vec::new();
    for p in points_of_scan {
        let mut best: Option<(f64, Goal)> = None;
        for (f, face) in faces.iter().enumerate() {
            if !among.get(f).copied().unwrap_or(false) {
                continue;
            }
            let (lo, hi) = boxes[f];
            if p.x < lo.x || p.y < lo.y || p.z < lo.z || p.x > hi.x || p.y > hi.y || p.z > hi.z {
                continue;
            }
            let Some(&(u, v, _)) = guesses[f].iter().min_by(|a, b| a.2.dist(*p).total_cmp(&b.2.dist(*p))) else { continue };
            let Some(foot) = foot_from(face, &nets[f], *p, u, v).filter(|f| f.inside) else { continue };
            let d = foot.point.dist(*p);
            if d <= reach && best.as_ref().is_none_or(|(b, _)| d < *b) {
                best = Some((d, Goal { face: f, u: foot.u, v: foot.v, to: *p, weight: 1.0 }));
            }
        }
        if let Some((_, g)) = best {
            goals.push(g);
        }
    }
    goals
}

/// A scan point under a face, for the deviation colouring: where it is, the face and its place
/// on it (carried from one look to the next, so a drag finds it again in a step or two).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Under {
    pub at: Vec3,
    pub face: usize,
    pub u: f64,
    pub v: f64,
}

/// The scan under the faces, each point on the face it lies nearest, and for each face which
/// way the scan's outside is (1 along the face's u × v, -1 against).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shade {
    pub points: Vec<Under>,
    pub outside: Vec<f64>,
    /// The scan's triangles under the faces, each with the point of `points` that colours it, so
    /// the colouring covers the scan as a surface rather than as dots. Empty when more than
    /// [`PAINT_LIMIT`] triangles lie under the faces: then the points alone are shown.
    pub paint: Vec<Painted>,
    /// Each of the scan's vertices under the faces with the point of `points` that colours it, for
    /// painting the scan's own vertices (no limit: 8 bytes each), and how many vertices the scan
    /// has.
    pub vertices: Vec<(u32, u32)>,
    pub vertex_count: usize,
}

/// A triangle of the scan under a face, coloured by one of the shade's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Painted {
    /// Its corners, nudged off the scan by a sliver of the scan's triangle size, so that it is
    /// drawn over the scan's own triangle rather than fighting it for the same depth. Each point
    /// is nudged once, along the scan's way out there (its triangles' normals together), so
    /// neighbours stay joined: nudged each along its own normal, a noisy scan's triangles came
    /// apart and the bare scan showed through the cracks.
    pub corners: [[f32; 3]; 3],
    /// The scan's way out at it (its corners' together), to light it by: lit by its own normal, a
    /// noisy scan's paint sparkles.
    pub up: [f32; 3],
    /// The index in [`Shade::points`] whose distance colours it.
    pub point: u32,
}

/// The most scan triangles a colouring paints (more and it is shown as dots).
pub const PAINT_LIMIT: usize = 250_000;

/// How far a painted point stands off the scan, as a share of a typical side of its triangles.
const PAINT_LIFT: f64 = 0.05;

/// The scan under the faces, at most `limit` points (the middles of its triangles, thinned evenly):
/// each on the face nearest it, its foot inside that face and within `reach` of it, facing the way
/// the face does (within 45°, on the side most of that face's points are, so neither a wall a
/// face's edge comes near nor the far side of a thin part counts). One look over the scan for all
/// the faces.
pub fn shade(scan: &crate::Scan, faces: &[NurbsSurface], reach: f64, limit: usize) -> Shade {
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    let Some(first) = nets.iter().flatten().next().copied() else { return Shade::default() };
    let (lo, hi) = nets.iter().flatten().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let pad = Vec3::new(reach, reach, reach);
    let (lo, hi) = (lo - pad, hi + pad);
    let boxed: Vec<(Vec3, Vec3, usize)> = (0..scan.triangle_count())
        .filter_map(|t| {
            let [a, b, c] = scan.corners(t)?;
            let m = (a + b + c) / 3.0;
            let inside = m.x >= lo.x && m.y >= lo.y && m.z >= lo.z && m.x <= hi.x && m.y <= hi.y && m.z <= hi.z;
            inside.then(|| (b - a).cross(c - a).normalized().map(|n| (m, n, t)))?
        })
        .collect();
    let near: Vec<(Vec3, Vec3)> = crate::thinned(&boxed, limit.saturating_mul(3)).into_iter().map(|(m, n, _)| (m, n)).collect();
    let middles: Vec<Vec3> = near.iter().map(|(m, _)| *m).collect();
    let all = vec![true; faces.len()];
    let found = feet(faces, &middles, reach, &all);
    // Each foot's facing: the triangle's normal against the face's there.
    let mut facing: Vec<(Under, f64)> = Vec::with_capacity(found.len());
    let by_place: HashMap<[u64; 3], Vec3> = near.iter().map(|(m, n)| ([m.x.to_bits(), m.y.to_bits(), m.z.to_bits()], *n)).collect();
    for g in found {
        let Some(n) = by_place.get(&[g.to.x.to_bits(), g.to.y.to_bits(), g.to.z.to_bits()]) else { continue };
        let Some((_, su, sv)) = frame(&faces[g.face], &nets[g.face], g.u, g.v) else { continue };
        let Some(normal) = su.cross(sv).normalized() else { continue };
        let along = n.dot(normal);
        if along.abs() >= std::f64::consts::FRAC_1_SQRT_2 {
            facing.push((Under { at: g.to, face: g.face, u: g.u, v: g.v }, along));
        }
    }
    let outside: Vec<f64> = (0..faces.len())
        .map(|f| if facing.iter().filter(|(u, _)| u.face == f).map(|(_, a)| a.signum()).sum::<f64>() < 0.0 { -1.0 } else { 1.0 })
        .collect();
    let kept: Vec<Under> = facing.into_iter().filter(|(u, a)| a.signum() == outside[u.face]).map(|(u, _)| u).collect();
    let points = crate::thinned(&kept, limit);
    // Each shade point stands for this many of the scan's triangles round it.
    let share = (boxed.len() as f64 / near.len().max(1) as f64) * (kept.len() as f64 / points.len().max(1) as f64);
    let (paint, vertices) = painted(scan, &boxed, &points, &by_place, share);
    Shade { points, outside, paint, vertices, vertex_count: scan.positions.len() }
}

/// The triangles of `boxed` (middle, normal, index) that a point of `points` stands for: the
/// nearest within the spacing the points have on the scan (`share` triangles each), facing the
/// same way as the triangle it was measured at (so the far side of a thin part is not painted).
/// Empty when there are more than [`PAINT_LIMIT`]. With them, each of their vertices and the
/// point that colours it, however many.
fn painted(
    scan: &crate::Scan,
    boxed: &[(Vec3, Vec3, usize)],
    points: &[Under],
    normals: &HashMap<[u64; 3], Vec3>,
    share: f64,
) -> (Vec<Painted>, Vec<(u32, u32)>) {
    if points.is_empty() || boxed.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // A typical side of the scan's triangles here, from an even sample of them.
    let sample = crate::thinned(boxed, 512);
    let sides: Vec<f64> = sample.iter().filter_map(|(_, _, t)| scan.corners(*t)).map(|[a, b, c]| (a.dist(b) + b.dist(c) + c.dist(a)) / 3.0).collect();
    let side = sides.iter().sum::<f64>() / sides.len().max(1) as f64;
    if !(side.is_finite() && side > 0.0) {
        return (Vec::new(), Vec::new());
    }
    // Within the points' own spacing: a triangle just past the face's side is not painted.
    let reach = side * 0.75 * share.max(1.0).sqrt();
    let at: Vec<Vec3> = points.iter().map(|u| u.at).collect();
    let grid = crate::poisson::PointGrid::new(&at, reach);
    let key = |p: Vec3| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    // Each point's way out, from the triangles under the faces round it (weighted by area), and
    // the one nudge every painted point gets. Only where there are few enough to be painted at
    // all (past that the paint is dropped; each triangle is then nudged along its own normal).
    let mut up: HashMap<u32, Vec3> = HashMap::new();
    for &(_, _, t) in boxed.iter().take(if boxed.len() <= 4 * PAINT_LIMIT { boxed.len() } else { 0 }) {
        let (Some([a, b, c]), Some(tri)) = (scan.corners(t), scan.triangles.get(t)) else { continue };
        let n = (b - a).cross(c - a);
        for &v in tri {
            *up.entry(v).or_insert(Vec3::ZERO) += n;
        }
    }
    let nudge = PAINT_LIFT * side;
    let out_at = |v: u32| up.get(&v).and_then(|n| n.normalized()).unwrap_or(Vec3::ZERO);
    let mut out = Vec::new();
    let mut whole = true;
    // The point colouring each vertex (u32::MAX: none).
    let mut by_vertex = vec![u32::MAX; scan.positions.len()];
    for &(m, n, t) in boxed {
        let mut best: Option<(f64, u32)> = None;
        grid.within(m, reach, |q, d| {
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, q));
            }
        });
        let Some((_, q)) = best else { continue };
        let facing = at.get(q as usize).and_then(|p| normals.get(&key(*p))).is_none_or(|sn| sn.dot(n) > 0.5);
        if !facing {
            continue;
        }
        let (Some([a, b, c]), Some(&tri)) = (scan.corners(t), scan.triangles.get(t)) else { continue };
        for v in &tri {
            if let Some(slot) = by_vertex.get_mut(*v as usize) {
                *slot = q;
            }
        }
        if !whole {
            continue;
        }
        let f = |p: Vec3, v: u32| {
            let p = p + if up.is_empty() { n } else { out_at(v) } * nudge;
            [p.x as f32, p.y as f32, p.z as f32]
        };
        let way = tri.iter().fold(Vec3::ZERO, |s, &v| s + out_at(v)).normalized().unwrap_or(n);
        out.push(Painted { corners: [f(a, tri[0]), f(b, tri[1]), f(c, tri[2])], up: [way.x as f32, way.y as f32, way.z as f32], point: q });
        if out.len() > PAINT_LIMIT {
            out = Vec::new();
            whole = false;
        }
    }
    let vertices = by_vertex.iter().enumerate().filter(|(_, q)| **q != u32::MAX).map(|(v, q)| (v as u32, *q)).collect();
    (out, vertices)
}

/// How far each point of `shade` is from the faces, along the way the scan's outside is (above
/// the surface positive): each point's place found again on its face from where it was. Only the
/// points on the faces `only` marks are measured again (all with `None`, or when `values` does not
/// hold one for each point yet); the others keep their values.
pub fn shade_values(shade: &mut Shade, faces: &[NurbsSurface], only: Option<&[bool]>, values: &mut Vec<f32>) {
    let nets: Vec<Vec<Vec3>> = faces.iter().map(points).collect();
    let all = only.is_none() || values.len() != shade.points.len();
    if values.len() != shade.points.len() {
        *values = vec![f32::NAN; shade.points.len()];
    }
    for (under, value) in shade.points.iter_mut().zip(values.iter_mut()) {
        if !all && !only.is_some_and(|o| o.get(under.face).copied().unwrap_or(false)) {
            continue;
        }
        let (Some(face), Some(net)) = (faces.get(under.face), nets.get(under.face)) else {
            *value = f32::NAN;
            continue;
        };
        if let Some(f) = foot_from(face, net, under.at, under.u, under.v) {
            under.u = f.u;
            under.v = f.v;
        }
        let Some((s, su, sv)) = frame(face, net, under.u, under.v) else {
            *value = f32::NAN;
            continue;
        };
        let n = su.cross(sv).normalized().unwrap_or(Vec3::Z) * shade.outside.get(under.face).copied().unwrap_or(1.0);
        *value = (under.at - s).dot(n) as f32;
    }
}

/// The network points of the faces `chosen` marks, and the rings `ring` deep round the network
/// points `around` in every face that has them.
pub fn region(faces: &[NurbsSurface], joins: &Joins, around: &[usize], ring: usize) -> Vec<bool> {
    let mut free = vec![false; joins.len()];
    for &p in around {
        for &(f, k) in joins.at.get(p).map_or(&[][..], Vec::as_slice) {
            let [nu, nv] = faces[f].count();
            let (i, j) = (k % nu, k / nu);
            for jj in j.saturating_sub(ring)..=(j + ring).min(nv - 1) {
                for ii in i.saturating_sub(ring)..=(i + ring).min(nu - 1) {
                    free[joins.of[f][jj * nu + ii]] = true;
                }
            }
        }
    }
    free
}

/// How many rings round a dot dragged move: enough that the dots beyond hold still (the surface
/// runs through every dot, which ripples out from one moved).
pub const DOT_RING: usize = 3;

/// The faces with one network point `point` (a dot) put at `to`, the points round it moved as
/// `ring` says: the points of the faces that hold it within `ring` of it move, the dots outside
/// that stay where they are (with `hold_near`, those inside too: the surface runs through every
/// other dot, QuickSurface's Don't Move Neighbours), the faces stay tangent across smooth seams.
pub fn dot_moved(faces: &[NurbsSurface], joins: &Joins, point: usize, to: Vec3, ring: usize, hold_near: bool) -> Option<Vec<NurbsSurface>> {
    dots_moved(faces, joins, &[(point, to)], ring, hold_near, &[])
}

/// As [`dot_moved`], several dots at once, with `extra` goals (scan places to fit).
pub fn dots_moved(
    faces: &[NurbsSurface],
    joins: &Joins,
    moves: &[(usize, Vec3)],
    ring: usize,
    hold_near: bool,
    extra: &[Goal],
) -> Option<Vec<NurbsSurface>> {
    let around: Vec<usize> = moves.iter().map(|m| m.0).collect();
    let free = region(faces, joins, &around, ring);
    let all = dots(faces, joins);
    let touched: Vec<bool> = (0..faces.len()).map(|f| joins.of[f].iter().any(|p| free[*p])).collect();
    // Each dot of a face that moves: a moved one where it goes, the rest held (inside the ring
    // only when holding the near ones too). A dot is held on each face it is on, so a seam's dot
    // stays on both.
    const HOLD: f64 = 1e3;
    let places: Vec<Vec<(f64, f64)>> = faces.iter().map(dot_places).collect();
    let mut goals: Vec<Goal> = extra.to_vec();
    for (p, dot) in all.iter().enumerate() {
        let to = moves.iter().find(|m| m.0 == p).map(|m| m.1);
        if to.is_none() && free[p] && !hold_near {
            continue;
        }
        let place = to.unwrap_or(dot.at);
        for &(f, k) in &joins.at[p] {
            if !touched[f] {
                continue;
            }
            let (u, v) = places[f].get(k).copied().unwrap_or((dot.u, dot.v));
            goals.push(Goal { face: f, u, v, to: place, weight: HOLD * if to.is_some() { 1e3 } else { 1.0 } });
        }
    }
    let even = if hold_near { 1e-3 } else { 1.0 };
    moved(faces, joins, &Moves { free: &free, goals: &goals, tangent: 40.0, even, bend: 0.0, also_smooth: &[] })
}

/// The network without face `face`, the rest as they were. Refused for the last face, and for one
/// whose going would leave the others in two pieces.
pub fn without_face(faces: &[NurbsSurface], face: usize) -> Result<Vec<NurbsSurface>, NetworkError> {
    if face >= faces.len() {
        return Err(NetworkError::Failed);
    }
    if faces.len() == 1 {
        return Err(NetworkError::LastFace);
    }
    let rest: Vec<NurbsSurface> = faces.iter().enumerate().filter(|(i, _)| *i != face).map(|(_, f)| f.clone()).collect();
    let joins = joins(&rest)?;
    if !connected(&rest, &joins) {
        return Err(NetworkError::WouldSplit);
    }
    Ok(rest)
}

/// The network smoothed one step (QuickSurface's Smooth): every point but the network's own
/// corners and the corners where faces meet unevenly moves to straighten its rows and columns,
/// held near the scan by `scan` (its places on the faces) or, with none, near where the surface is
/// (its dots), the faces tangent where they met smoothly. `strength` above 0: 1 smooths hard.
pub fn smoothed(faces: &[NurbsSurface], joins: &Joins, scan: &[Goal], strength: f64) -> Option<Vec<NurbsSurface>> {
    if !(strength.is_finite() && strength > 0.0) {
        return None;
    }
    let held: Vec<usize> = joins.corners.iter().filter(|c| c.irregular() || (c.border && c.faces == 1)).map(|c| c.point).collect();
    let free: Vec<bool> = (0..joins.len()).map(|p| !held.contains(&p)).collect();
    let goals: Vec<Goal> = if scan.is_empty() {
        let places: Vec<Vec<(f64, f64)>> = faces.iter().map(dot_places).collect();
        dots(faces, joins)
            .iter()
            .enumerate()
            .flat_map(|(p, d)| joins.at[p].iter().map(move |&(f, k)| (f, k, d.at)))
            .filter_map(|(f, k, at)| places.get(f)?.get(k).map(|&(u, v)| Goal { face: f, u, v, to: at, weight: 1.0 }))
            .collect()
    } else {
        scan.to_vec()
    };
    let bend = strength * strength;
    moved(faces, joins, &Moves { free: &free, goals: &goals, tangent: 40.0, even: 0.0, bend, also_smooth: &[] })
}

/// The network under a smoothing brush (QuickSurface's): the points `under` it, each with how much
/// of the brush it gets (0 to 1, more toward the brush's middle), move to straighten their rows and
/// columns, as hard as `strength` (0 to 1); held near the scan by `scan` (its places on the faces)
/// or, with none, near where the surface is (its dots), and each near its own dot the more the less
/// brush it gets, so the brush's middle smooths most and its rim hardly moves. The network's own
/// corners and the corners where faces meet unevenly stay, and the faces stay tangent where they
/// met smoothly. `None` when nothing under the brush may move.
pub fn brushed(faces: &[NurbsSurface], joins: &Joins, under: &[(usize, f64)], scan: &[Goal], strength: f64) -> Option<Vec<NurbsSurface>> {
    if !(strength.is_finite() && strength > 0.0) || faces.len() != joins.of.len() {
        return None;
    }
    let held: Vec<usize> = joins.corners.iter().filter(|c| c.irregular() || (c.border && c.faces == 1)).map(|c| c.point).collect();
    let mut free = vec![false; joins.len()];
    let mut share = vec![0.0f64; joins.len()];
    for &(p, w) in under {
        if p < joins.len() && w.is_finite() && w > 0.0 && !held.contains(&p) {
            free[p] = true;
            share[p] = share[p].max(w.min(1.0));
        }
    }
    if !free.iter().any(|f| *f) {
        return None;
    }
    let places: Vec<Vec<(f64, f64)>> = faces.iter().map(dot_places).collect();
    let all = dots(faces, joins);
    let mut goals: Vec<Goal> = scan.to_vec();
    for p in (0..joins.len()).filter(|p| free[*p]) {
        let hold = 2.0 * (1.0 - share[p]) + 0.05;
        for &(f, k) in joins.at.get(p).map_or(&[][..], Vec::as_slice) {
            let (Some(&(u, v)), Some(dot)) = (places.get(f).and_then(|pl| pl.get(k)), all.get(p)) else { continue };
            goals.push(Goal { face: f, u, v, to: dot.at, weight: hold });
        }
    }
    // The straightening weighs as much as `strength`² of one goal on each row through a point
    // (not as much as all its goals, as `moved` scales a row): a bump is brushed down a little a
    // step, and the scan's places, or the dots, keep the surface's own curve.
    let n_free = free.iter().filter(|f| **f).count() as f64;
    let mut weights: Vec<f64> = goals.iter().map(|g| g.weight).collect();
    weights.sort_by(f64::total_cmp);
    let middle = weights.get(weights.len() / 2).copied().unwrap_or(1.0);
    let per_point = (middle * weights.len() as f64 / n_free).max(1.0);
    let bend = strength.min(1.0).powi(2) / per_point;
    moved(faces, joins, &Moves { free: &free, goals: &goals, tangent: 40.0, even: 0.0, bend, also_smooth: &[] })
}

/// The network's faces with every face's points doubled both ways (Increase Resolution): every
/// face exactly as it was, so seams stay seams.
pub fn refined(faces: &[NurbsSurface]) -> Result<Vec<NurbsSurface>, crate::net::NetError> {
    faces.iter().map(|f| crate::net::refine(f, true, true)).collect()
}

/// The places across a new face snapped to the scan, rounds of fitting it alone and then held
/// tangent to its neighbours.
const ADD_PLACES: usize = 4;
const ADD_FREE_ROUNDS: usize = 3;
const ADD_ROUNDS: usize = 4;

/// A new face fitted to the scan on its own that meets a neighbour within this, degrees, goes on
/// from it smoothly (the scan does) and is held tangent to it; at a larger angle the scan has a
/// sharp edge there, and the seam is left a crease.
pub const CONTINUES_DEGREES: f64 = 25.0;

/// A face corner whose two sides meet within this of a straight line (or doubled back on each
/// other), degrees, is flat: refused.
const FLAT_DEGREES: f64 = 5.0;

/// The network with a face added on its open side `side` of `face`, out to the far corners `far`
/// (in either order), `rows` points across it: its near side the side it is added on (one with it),
/// a side of it that runs where an open side of another face does (one point for point) one with
/// that too, and the rest fitted to the scan by `snap` (`None` for a place with no scan near). It is
/// fitted on its own first; across each side it shares where it then goes on smoothly from the face
/// there (within [`CONTINUES_DEGREES`]) it is held tangent to it, the row of that face next to the
/// side refitted with it, and where the scan turns sharply the seam is left a crease. The new face
/// is the last. Refused when it would have a flat corner.
pub fn with_face(
    faces: &[NurbsSurface],
    joins: &Joins,
    face: usize,
    side: Side,
    far: [Vec3; 2],
    rows: usize,
    snap: impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>>,
) -> Result<Vec<NurbsSurface>, NetworkError> {
    if faces.len() >= MOST_FACES {
        return Err(NetworkError::TooManyFaces);
    }
    if !joins.open.contains(&(face, side)) {
        return Err(NetworkError::NotOpen);
    }
    if far.iter().any(|p| !p.is_finite()) {
        return Err(NetworkError::Failed);
    }
    let rows = rows.clamp(crate::free_form::LEAST_POINTS, crate::free_form::MOST_POINTS);
    let places = network_points(faces, joins);
    let ids = joins.side(faces, face, side);
    let m = ids.len();
    if m < 4 {
        return Err(NetworkError::TooShort);
    }
    let edge: Vec<Vec3> = ids.iter().map(|p| places[*p]).collect();
    let (first, last) = (edge[0], edge[m - 1]);
    let far = if first.dist(far[0]) + last.dist(far[1]) <= first.dist(far[1]) + last.dist(far[0]) { far } else { [far[1], far[0]] };
    // The far corners on the scan, unless one is a point of the network already (it is welded).
    let snapped = snap(&far, None);
    let far: [Vec3; 2] = std::array::from_fn(|k| {
        let on_net = places.iter().find(|p| p.dist(far[k]) <= 1e-9 * (1.0 + first.dist(last)));
        on_net.copied().or_else(|| snapped.get(k).copied().flatten()).unwrap_or(far[k])
    });
    if far[0].dist(first) <= 1e-9 || far[1].dist(last) <= 1e-9 {
        return Err(NetworkError::Failed);
    }
    // The net: rows from the edge out to the far side, u along the edge.
    let mut net = vec![Vec3::ZERO; m * rows];
    for r in 0..rows {
        let t = r as f64 / (rows - 1) as f64;
        for k in 0..m {
            let out = far[0] + (far[1] - far[0]) * (k as f64 / (m - 1) as f64);
            net[r * m + k] = edge[k] + (out - edge[k]) * t;
        }
    }
    let make = |net: &[Vec3]| NurbsSurface::new([3, 3], [clamped_knots(m), clamped_knots(rows)], net, &vec![1.0; net.len()]).ok();
    // Facing the way the face it grows from does.
    let grows_from = &faces[face];
    let (facing_from, facing_new) = (side_normal(grows_from, side), make(&net).and_then(|s| side_normal(&s, Side::FirstRow)));
    if let (Some(a), Some(b)) = (facing_from, facing_new)
        && a.dot(b) < 0.0
    {
        for r in 0..rows {
            net[r * m..(r + 1) * m].reverse();
        }
    }
    // The ruled net put on the scan at its nearest places, its edge row kept: the sides it runs
    // out along and its far side start where the scan is square under them (a sphere's great
    // circles). Put on along the flat net's one facing instead, they would be a plane's cut
    // through a part that curves round, which the fit along the face's own normals then keeps.
    let found = snap(&net[m..], None);
    for (p, q) in net[m..].iter_mut().zip(found) {
        if let Some(q) = q {
            *p = q;
        }
    }
    let new = make(&net).ok_or(NetworkError::Failed)?;
    // Its other sides one with open sides of the network that run between the same points.
    let mut held = vec![false; m * rows];
    for k in Side::FirstRow.indices([m, rows]) {
        held[k] = true;
    }
    for s in [Side::LastRow, Side::FirstColumn, Side::LastColumn] {
        let mine = s.indices([m, rows]);
        let (a, b) = (net[mine[0]], net[mine[mine.len() - 1]]);
        let found = joins.open.iter().filter(|o| **o != (face, side)).find_map(|&(f, os)| {
            let theirs: Vec<Vec3> = joins.side(faces, f, os).iter().map(|p| places[*p]).collect();
            let close = |p: Vec3, q: Vec3| p.dist(q) <= 1e-9 * (1.0 + first.dist(last));
            (theirs.len() == mine.len()).then_some(())?;
            if close(theirs[0], a) && close(theirs[theirs.len() - 1], b) {
                Some(theirs)
            } else if close(theirs[0], b) && close(theirs[theirs.len() - 1], a) {
                Some(theirs.into_iter().rev().collect())
            } else {
                None
            }
        });
        if let Some(theirs) = found {
            for (k, p) in mine.iter().zip(theirs) {
                net[*k] = p;
                held[*k] = true;
            }
        }
    }
    for k in corner_places([m, rows]) {
        held[k] = true;
    }
    let new = with_points(&new, &net).ok_or(NetworkError::Failed)?;
    let mut all: Vec<NurbsSurface> = faces.to_vec();
    all.push(new);
    let mut grown = joins_after(&all)?;
    let at = all.len() - 1;
    let free_of_new = |grown: &Joins, beside: &[usize]| -> Vec<bool> {
        (0..grown.len()).map(|p| beside.contains(&p) || grown.at[p].iter().all(|&(f, k)| f == at && !held[k])).collect()
    };
    // First on its own, held tangent to nothing (its neighbours' own tangency stays).
    for round in 0..ADD_FREE_ROUNDS {
        let goals = snapped_places(&all, &[at], &snap);
        let mut alone = grown.clone();
        alone.smooth.retain(|t| ![t.e, t.a, t.b].iter().any(|p| grown.at[*p].iter().all(|&(f, _)| f == at)));
        let free = free_of_new(&alone, &[]);
        let bend = if round == 0 { 1e-2 } else { 1e-4 };
        let moves = Moves { free: &free, goals: &goals, tangent: 40.0, even: 0.0, bend, also_smooth: &[] };
        match moved(&all, &alone, &moves) {
            Some(next) => all = next,
            None => break,
        }
        grown = joins_after(&all)?;
    }
    // Then tangent across the sides it shares where it goes on smoothly from the face there.
    let smooth_seams: Vec<usize> = (0..grown.seams.len())
        .filter(|&i| {
            let seam = grown.seams[i];
            (seam.a == at || seam.b == at) && seam.a != seam.b && seam_angle(&all, &seam) <= CONTINUES_DEGREES
        })
        .collect();
    let tangent: Vec<Across> = tangency_across(&all, &grown, at).into_iter().filter(|t| smooth_seams.contains(&t.seam)).collect();
    // The row of the face it grows from next to such a side moves too, fitted to the scan with the
    // new face: so the two settle into one smooth surface across it, even where that face's net
    // turned back at its edge (going straight on from there, the new face would fold).
    let beside: Vec<usize> =
        tangent.iter().flat_map(|t| [t.a, t.b]).filter(|p| grown.at[*p].len() == 1 && grown.at[*p].iter().all(|&(f, _)| f != at)).collect();
    let mut fitted_faces: Vec<usize> = beside.iter().flat_map(|p| grown.at[*p].iter().map(|&(f, _)| f)).collect();
    fitted_faces.push(at);
    fitted_faces.sort_unstable();
    fitted_faces.dedup();
    if !tangent.is_empty() {
        // Creased sides stay out of the solve's tangency, as they do on reading the network again.
        for round in 0..ADD_ROUNDS {
            let goals = snapped_places(&all, &fitted_faces, &snap);
            let mut held_smooth = grown.clone();
            held_smooth
                .smooth
                .retain(|t| !grown.seams.get(t.seam).is_some_and(|seam| (seam.a == at || seam.b == at) && !smooth_seams.contains(&t.seam)));
            let free = free_of_new(&held_smooth, &beside);
            let bend = if round == 0 { 1e-3 } else { 1e-4 };
            let moves = Moves { free: &free, goals: &goals, tangent: 40.0, even: 0.0, bend, also_smooth: &tangent };
            match moved(&all, &held_smooth, &moves) {
                Some(next) => all = next,
                None => break,
            }
            grown = joins_after(&all)?;
        }
    }
    for &f in &fitted_faces {
        if all.get(f).is_some_and(|s| flattest_corner(s) < FLAT_DEGREES) {
            return Err(NetworkError::FlatCorner);
        }
    }
    Ok(all)
}

/// The joins of `faces` (each weld exact, as a face added copies its points).
fn joins_after(faces: &[NurbsSurface]) -> Result<Joins, NetworkError> {
    joins(faces)
}

/// Places across the faces `which` put on the scan by `snap` along the way the face faces at each
/// place (turned as the face faces as a whole), never across it (on a rough scan the nearest point
/// is often off to one side, which would drag the places, and the net with them, across the face):
/// goals of weight 1. Along the face's own normal, not one way for the whole face: a face that
/// curves round (a sixth of a sphere) has its sides put where the scan is square under them, so
/// they run as the scan's own lines do (a sphere's great circles), not as a plane's cut through it,
/// which runs on smoothly into the next face's side and leaves the face closing that corner flat.
fn snapped_places(all: &[NurbsSurface], which: &[usize], snap: &impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>>) -> Vec<Goal> {
    let mut goals = Vec::new();
    for &f in which {
        let Some(surface) = all.get(f) else { continue };
        let snet = points(surface);
        let [cu, cv] = surface.count();
        let (nu, nv) = (ADD_PLACES * cu, ADD_PLACES * cv);
        let [[u0, u1], [v0, v1]] = surface.domain();
        let mut uv = Vec::with_capacity((nu + 1) * (nv + 1));
        let mut on = Vec::with_capacity(uv.capacity());
        for j in 0..=nv {
            for i in 0..=nu {
                let (u, v) = (u0 + (u1 - u0) * i as f64 / nu as f64, v0 + (v1 - v0) * j as f64 / nv as f64);
                if let Some((p, su, sv)) = frame(surface, &snet, u, v) {
                    uv.push((u, v));
                    on.push((p, su.cross(sv).normalized()));
                }
            }
        }
        let whole = mean_normal(surface);
        for (&(u, v), (p, n)) in uv.iter().zip(on) {
            let along = match (n, whole) {
                (Some(n), Some(w)) if n.dot(w) < 0.0 => Some(-n),
                (Some(n), _) => Some(n),
                (None, w) => w,
            };
            if let Some(to) = snap(&[p], along).into_iter().next().flatten() {
                goals.push(Goal { face: f, u, v, to, weight: 1.0 });
            }
        }
    }
    goals
}

/// The normal of `s` at `t` (0 to 1) along its side `side`.
fn normal_along(s: &NurbsSurface, side: Side, t: f64) -> Option<Vec3> {
    let net = points(s);
    let [[u0, u1], [v0, v1]] = s.domain();
    let (u, v) = match side {
        Side::FirstRow => (u0 + (u1 - u0) * t, v0),
        Side::LastRow => (u0 + (u1 - u0) * t, v1),
        Side::FirstColumn => (u0, v0 + (v1 - v0) * t),
        Side::LastColumn => (u1, v0 + (v1 - v0) * t),
    };
    let (_, su, sv) = frame(s, &net, u, v)?;
    su.cross(sv).normalized()
}

/// The angle the two faces of `seam` meet at, degrees: between their normals, on the average along
/// it (0 for faces that go on smoothly from each other).
pub fn seam_angle(faces: &[NurbsSurface], seam: &Seam) -> f64 {
    let (Some(a), Some(b)) = (faces.get(seam.a), faces.get(seam.b)) else { return 0.0 };
    let angles: Vec<f64> = (1..8)
        .filter_map(|k| {
            let t = f64::from(k) / 8.0;
            let na = normal_along(a, seam.a_side, t)?;
            let nb = normal_along(b, seam.b_side, if seam.reversed { 1.0 - t } else { t })?;
            Some(na.dot(nb).abs().clamp(-1.0, 1.0).acos().to_degrees())
        })
        .collect();
    if angles.is_empty() { 0.0 } else { angles.iter().sum::<f64>() / angles.len() as f64 }
}

/// How near a straight line the flattest corner of `s` is, degrees: at each corner the angle between
/// its two sides there, the least of its distances from 0° and 180°.
pub fn flattest_corner(s: &NurbsSurface) -> f64 {
    let net = points(s);
    let [[u0, u1], [v0, v1]] = s.domain();
    let mut flattest = 90.0f64;
    for (u, v) in [(u0, v0), (u1, v0), (u1, v1), (u0, v1)] {
        let Some((_, su, sv)) = frame(s, &net, u, v) else { continue };
        let (Some(a), Some(b)) = (su.normalized(), sv.normalized()) else {
            flattest = 0.0;
            continue;
        };
        let angle = a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
        flattest = flattest.min(angle).min(180.0 - angle);
    }
    flattest
}

/// The tangency rows across every seam of face `at`, smooth or not yet.
fn tangency_across(faces: &[NurbsSurface], joins: &Joins, at: usize) -> Vec<Across> {
    (0..joins.seams.len()).filter(|&i| joins.seams[i].a == at || joins.seams[i].b == at).flat_map(|i| across_seam(faces, joins, i)).collect()
}

/// Across seam `seam`: each point on it and the points next to it either side, as tangency across
/// it holds them (whether or not the faces meet smoothly there now); at its ends only where
/// [`end_held`] holds them.
pub fn across_seam(faces: &[NurbsSurface], joins: &Joins, seam: usize) -> Vec<Across> {
    let irregular: std::collections::HashSet<usize> = joins.irregular().map(|c| c.point).collect();
    let border: std::collections::HashSet<usize> = joins.corners.iter().filter(|c| c.border).map(|c| c.point).collect();
    let Some(s) = joins.seams.get(seam) else { return Vec::new() };
    let (Some(fa), Some(fb), Some(oa), Some(ob)) = (faces.get(s.a), faces.get(s.b), joins.of.get(s.a), joins.of.get(s.b)) else {
        return Vec::new();
    };
    let (ca, cb) = (fa.count(), fb.count());
    let (ka, mut kb) = (s.a_side.indices(ca), s.b_side.indices(cb));
    if s.reversed {
        kb.reverse();
    }
    let mut out = Vec::new();
    for (n, (&pa, &pb)) in ka.iter().zip(&kb).enumerate() {
        let (Some(&e), Some(&ia), Some(&ib)) = (oa.get(pa), oa.get(inward(s.a_side, ca, pa)), ob.get(inward(s.b_side, cb, pb))) else { continue };
        if n == 0 || n + 1 == ka.len() {
            let (na, nb) = (points(fa), points(fb));
            let (Some(&pe), Some(&pia), Some(&pib)) = (na.get(pa), na.get(inward(s.a_side, ca, pa)), nb.get(inward(s.b_side, cb, pb))) else {
                continue;
            };
            if !end_held(irregular.contains(&e), border.contains(&e), pia, pe, pib) {
                continue;
            }
        }
        if ia != ib {
            out.push(Across { seam, e, a: ia, b: ib });
        }
    }
    out
}

/// Which way round each face of a network runs, as one surface: +1 for a face whose normal is
/// the first face's way, -1 for one turned over against it. Read from the seams, not the normals:
/// two faces agree when their shared side runs opposite ways round them (as on any surface that
/// has two sides), which holds at a crease of 90° as well as on a smooth seam. A face no seam
/// reaches keeps +1.
fn orientations(faces: &[NurbsSurface], joins: &Joins) -> Vec<f64> {
    // How a side runs round its face, counterclockwise in (u, v), against its points' order.
    let round = |side: Side| -> f64 {
        match side {
            Side::FirstRow | Side::LastColumn => 1.0,
            Side::LastRow | Side::FirstColumn => -1.0,
        }
    };
    let mut way: Vec<Option<f64>> = vec![None; faces.len()];
    for start in 0..faces.len() {
        if way[start].is_some() {
            continue;
        }
        way[start] = Some(1.0);
        let mut todo = vec![start];
        while let Some(f) = todo.pop() {
            let Some(here) = way[f] else { continue };
            for seam in &joins.seams {
                let (other, mine, theirs) = if seam.a == f {
                    (seam.b, seam.a_side, seam.b_side)
                } else if seam.b == f {
                    (seam.a, seam.b_side, seam.a_side)
                } else {
                    continue;
                };
                if way[other].is_some() {
                    continue;
                }
                let along = if seam.reversed { -1.0 } else { 1.0 };
                let agree = round(mine) * round(theirs) * along < 0.0;
                way[other] = Some(if agree { here } else { -here });
                todo.push(other);
            }
        }
    }
    way.into_iter().map(|w| w.unwrap_or(1.0)).collect()
}

/// The network `faces` moved `distance` along its normals (the first face's way, the others
/// turned to agree with it): one network of the same faces, points and seams, fitted as a whole
/// to places across each face moved that far along its normal there, so its faces share their
/// sides as the network's do and stay tangent where it is. `None` when it cannot be made.
fn offset_network(faces: &[NurbsSurface], joins: &Joins, distance: f64) -> Option<Vec<NurbsSurface>> {
    if distance == 0.0 {
        return Some(faces.to_vec());
    }
    let way = orientations(faces, joins);
    let normal_at = |f: usize, u: f64, v: f64| -> Option<(Vec3, Vec3)> {
        let face = faces.get(f)?;
        let (p, su, sv) = frame(face, &points(face), u, v)?;
        Some((p, su.cross(sv).normalized()? * way[f]))
    };
    // A first guess: each point moved along the normals at its places' Greville points.
    let start = network_points(faces, joins);
    let mut sum = vec![Vec3::ZERO; start.len()];
    for (f, face) in faces.iter().enumerate() {
        for (k, (u, v)) in greville_places(face).into_iter().enumerate() {
            if let (Some(&p), Some((_, n))) = (joins.of.get(f).and_then(|of| of.get(k)), normal_at(f, u, v)) {
                sum[p] += n;
            }
        }
    }
    let guess: Vec<Vec3> = start.iter().zip(&sum).map(|(p, n)| *p + n.normalized().unwrap_or(Vec3::ZERO) * distance).collect();
    let mut moved_faces = with_network_points(faces, joins, &guess)?;
    // Places across each face, moved along its normal: where the offset surface runs.
    let mut goals = Vec::new();
    for (f, face) in faces.iter().enumerate() {
        let [cu, cv] = face.count();
        let (nu, nv) = (ADD_PLACES * cu, ADD_PLACES * cv);
        let [[u0, u1], [v0, v1]] = face.domain();
        for j in 0..=nv {
            for i in 0..=nu {
                let (u, v) = (u0 + (u1 - u0) * i as f64 / nu as f64, v0 + (v1 - v0) * j as f64 / nv as f64);
                if let Some((p, n)) = normal_at(f, u, v) {
                    goals.push(Goal { face: f, u, v, to: p + n * distance, weight: 1.0 });
                }
            }
        }
    }
    let free = vec![true; joins.len()];
    for _ in 0..OFFSET_ROUNDS {
        let moves = Moves { free: &free, goals: &goals, tangent: 40.0, even: 0.0, bend: 1e-6, also_smooth: &[] };
        moved_faces = moved(&moved_faces, joins, &moves)?;
    }
    Some(moved_faces)
}

/// Rounds of the whole offset network's fit (the seams' ratios are read again each round).
const OFFSET_ROUNDS: usize = 2;

/// A strip of four rows ruled between two sides of the same points' count (`near` and `far`, in
/// order), a cubic in both ways on `knots` along them: it shares each side exactly with the face
/// it is, and its short sides with the strips beside it.
fn ruled_strip(near: &[Vec3], far: &[Vec3], knots: &[f64]) -> Option<NurbsSurface> {
    let m = near.len();
    if m < 4 || far.len() != m {
        return None;
    }
    let rows = 4;
    let net: Vec<Vec3> = (0..rows).flat_map(|r| (0..m).map(move |k| near[k] + (far[k] - near[k]) * (r as f64 / (rows - 1) as f64))).collect();
    NurbsSurface::new([3, 3], [knots.to_vec(), clamped_knots(rows)], &net, &vec![1.0; net.len()]).ok()
}

/// The network `faces` thickened into the faces of a solid: the network moved `near` along its
/// normals (0: the network itself) and moved `far`, and a ruled strip along each open side
/// between the two; each strip shares its sides exactly with them and with the strips beside it,
/// so the faces sew shut. The normals are the first face's way, the others turned to agree. A
/// network with no open side (it closes on itself) is refused: what it would make is two shells.
/// Made from the faces' own nets, so the kernel's offset of the faces (which leaves the
/// corners where faces meet a hair apart) is not needed.
pub fn thickened(faces: &[NurbsSurface], near: f64, far: f64) -> Result<Vec<NurbsSurface>, NetworkError> {
    if !(near.is_finite() && far.is_finite()) || (far - near).abs() <= 0.0 {
        return Err(NetworkError::Failed);
    }
    let joins = joins(faces)?;
    if joins.open.is_empty() {
        return Err(NetworkError::Closed);
    }
    let inner = offset_network(faces, &joins, near).ok_or(NetworkError::Failed)?;
    let outer = offset_network(faces, &joins, far).ok_or(NetworkError::Failed)?;
    let (a, b) = (network_points(&inner, &joins), network_points(&outer, &joins));
    let mut out = inner;
    out.extend(outer);
    for &(f, side) in &joins.open {
        let ids = joins.side(faces, f, side);
        let knots = faces.get(f).map(|s| s.knots()[side.along()].clone()).ok_or(NetworkError::Failed)?;
        let near_side: Vec<Vec3> = ids.iter().map(|p| a[*p]).collect();
        let far_side: Vec<Vec3> = ids.iter().map(|p| b[*p]).collect();
        out.push(ruled_strip(&near_side, &far_side, &knots).ok_or(NetworkError::Failed)?);
    }
    Ok(out)
}

/// Whether a smooth seam's end at a corner is held in a line with the points next to it either
/// side (`a`, the corner `e`, `b`): never at an irregular corner (its faces would pull against each
/// other), always at a grid's corner inside the network, and at a corner on its edge only where the
/// two faces' open sides there run on from each other within [`CONTINUES_DEGREES`] (a grid's edge:
/// a panel of faces side by side, or a corner a fourth face will make a grid's). A corner on the edge
/// whose sides turn there is left free: the faces still to come close it as one of three (a cube's
/// corner on a sphere, a limb's end), and held in a line before they do it would be a flat corner of
/// the face that closes it.
fn end_held(irregular: bool, border: bool, a: Vec3, e: Vec3, b: Vec3) -> bool {
    if irregular {
        return false;
    }
    if !border {
        return true;
    }
    match ((e - a).normalized(), (b - e).normalized()) {
        (Some(da), Some(db)) => da.dot(db) >= CONTINUES_DEGREES.to_radians().cos(),
        _ => false,
    }
}

/// Whether each seam is a crease: the faces are not held tangent across it (they met sharply when
/// the network was read, or it was creased).
pub fn creased(joins: &Joins) -> Vec<bool> {
    let held: std::collections::HashSet<usize> = joins.smooth.iter().map(|t| t.seam).collect();
    (0..joins.seams.len()).map(|i| !held.contains(&i)).collect()
}

/// `joins` with seam `seam` a crease: the faces no longer held tangent across it.
pub fn without_tangency(joins: &Joins, seam: usize) -> Joins {
    let mut j = joins.clone();
    j.smooth.retain(|t| t.seam != seam);
    j
}

/// `joins` with seam `seam` smooth: the faces held tangent across it from now on.
pub fn with_tangency(faces: &[NurbsSurface], joins: &Joins, seam: usize) -> Joins {
    let mut j = without_tangency(joins, seam);
    j.smooth.extend(across_seam(faces, joins, seam));
    j
}

/// The ends of seam `seam`: its first and last point.
pub fn seam_ends(faces: &[NurbsSurface], joins: &Joins, seam: usize) -> Option<(Vec3, Vec3)> {
    let s = joins.seams.get(seam)?;
    let ids = joins.side(faces, s.a, s.a_side);
    let places = network_points(faces, joins);
    Some((*places.get(*ids.first()?)?, *places.get(*ids.last()?)?))
}

/// The seam whose ends are `a` and `b` (either way round), each within `tolerance`.
pub fn seam_between(faces: &[NurbsSurface], joins: &Joins, a: Vec3, b: Vec3, tolerance: f64) -> Option<usize> {
    if !(a.is_finite() && b.is_finite() && tolerance.is_finite()) {
        return None;
    }
    (0..joins.seams.len())
        .filter_map(|i| {
            let (p, q) = seam_ends(faces, joins, i)?;
            let off = (p.dist(a) + q.dist(b)).min(p.dist(b) + q.dist(a));
            (off <= 2.0 * tolerance).then_some((i, off))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1))
        .map(|(i, _)| i)
}

/// The seam nearest `p` (sampled along each), and how far it is.
pub fn seam_near(faces: &[NurbsSurface], joins: &Joins, p: Vec3) -> Option<(usize, f64)> {
    if !p.is_finite() {
        return None;
    }
    (0..joins.seams.len())
        .filter_map(|i| {
            let s = joins.seams.get(i)?;
            let f = faces.get(s.a)?;
            let d = (0..=16).filter_map(|k| s.a_side.at(f, f64::from(k) / 16.0)).map(|q| q.dist(p)).fold(f64::INFINITY, f64::min);
            d.is_finite().then_some((i, d))
        })
        .min_by(|x, y| x.1.total_cmp(&y.1))
}

/// How many rows of points either side of a seam a crease, or its removal, refits: the seam's
/// points and the rows next to them.
pub const CREASE_RING: usize = 2;

/// The network with the points within [`CREASE_RING`] of seam `seam` fitted to `scan` again (its
/// points on the faces, within `reach`), the faces held tangent as `joins` says ([`without_tangency`]
/// for a crease, [`with_tangency`] for a seam made smooth); the network's own corners, its open
/// sides, irregular corners and the points `held` marks stay. With no scan points the dots of the
/// points that move are held where they are, so the faces change only to meet as `joins` says.
/// `None` when nothing may move or the solve has no answer.
pub fn refit_near_seam(faces: &[NurbsSurface], joins: &Joins, seam: usize, scan: &[Vec3], reach: f64, held: &[bool]) -> Option<Vec<NurbsSurface>> {
    let s = joins.seams.get(seam)?;
    if faces.len() != joins.of.len() || !(reach.is_finite() && reach > 0.0) {
        return None;
    }
    let on = joins.side(faces, s.a, s.a_side);
    let mut free = region(faces, joins, &on, CREASE_RING);
    for c in joins.corners.iter().filter(|c| c.irregular() || (c.border && c.faces == 1)) {
        if let Some(f) = free.get_mut(c.point) {
            *f = false;
        }
    }
    for &(f, side) in &joins.open {
        for p in joins.side(faces, f, side) {
            if let Some(f) = free.get_mut(p) {
                *f = false;
            }
        }
    }
    for (p, h) in held.iter().enumerate() {
        if *h && let Some(f) = free.get_mut(p) {
            *f = false;
        }
    }
    if !free.iter().any(|f| *f) {
        return None;
    }
    let touched: Vec<bool> = joins.of.iter().map(|ids| ids.iter().any(|p| free.get(*p).copied().unwrap_or(false))).collect();
    let mut faces = faces.to_vec();
    let rounds = if scan.is_empty() { 1 } else { 3 };
    for round in 0..rounds {
        let goals: Vec<Goal> = if scan.is_empty() {
            let places: Vec<Vec<(f64, f64)>> = faces.iter().map(dot_places).collect();
            let all = dots(&faces, joins);
            (0..joins.len())
                .filter(|p| free[*p])
                .flat_map(|p| joins.at[p].iter().map(move |&(f, k)| (f, k, p)))
                .filter_map(|(f, k, p)| {
                    let &(u, v) = places.get(f)?.get(k)?;
                    Some(Goal { face: f, u, v, to: all.get(p)?.at, weight: 1.0 })
                })
                .collect()
        } else {
            feet(&faces, scan, reach, &touched)
        };
        // Hardly held even: the rows next to the seam must move apart from it for a crease.
        let moves = Moves { free: &free, goals: &goals, tangent: 40.0, even: 1e-3, bend: 1e-4, also_smooth: &[] };
        match moved(&faces, joins, &moves) {
            Some(next) => faces = next,
            None if round > 0 => break,
            None => return None,
        }
    }
    Some(faces)
}

/// Whether the edge with ends `a` and `b` is `e` (either way round), each end within `tolerance`.
fn same_ends(e: &(Vec3, Vec3), a: Vec3, b: Vec3, tolerance: f64) -> bool {
    (e.0.dist(a) <= tolerance && e.1.dist(b) <= tolerance) || (e.0.dist(b) <= tolerance && e.1.dist(a) <= tolerance)
}

/// How far two ends may be apart and still name the same seam: a millionth of the network's size.
pub fn ends_tolerance(faces: &[NurbsSurface]) -> f64 {
    let all: Vec<Vec3> = faces.iter().flat_map(points).collect();
    let Some(first) = all.first().copied() else { return 1e-9 };
    let (lo, hi) = all.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    (1e-6 * hi.dist(lo)).max(1e-9)
}

/// How `faces` join ([`joins`]), with the seams creased by hand (`creases`, each by its ends) held
/// creases and those made smooth by hand (`smooths`) held tangent, whatever angle the faces meet at
/// now: a seam's ends within `tolerance` of a kept edge's are that edge.
pub fn joins_keeping(faces: &[NurbsSurface], creases: &[(Vec3, Vec3)], smooths: &[(Vec3, Vec3)], tolerance: f64) -> Result<Joins, NetworkError> {
    let mut joins = joins(faces)?;
    if creases.is_empty() && smooths.is_empty() {
        return Ok(joins);
    }
    for i in 0..joins.seams.len() {
        let Some((a, b)) = seam_ends(faces, &joins, i) else { continue };
        if creases.iter().any(|e| same_ends(e, a, b, tolerance)) {
            joins = without_tangency(&joins, i);
        } else if smooths.iter().any(|e| same_ends(e, a, b, tolerance)) {
            joins = with_tangency(faces, &joins, i);
        }
    }
    Ok(joins)
}

/// The seams whose join `joins` holds otherwise than the faces' angle reads it ([`joins`]), each by
/// its ends: those creased though the faces meet smoothly, and those held tangent though they meet
/// at an angle. What a design keeps of a Free Form besides its faces.
pub fn held_by_hand(faces: &[NurbsSurface], joins: &Joins) -> (Vec<(Vec3, Vec3)>, Vec<(Vec3, Vec3)>) {
    let Ok(read) = self::joins(faces) else { return (Vec::new(), Vec::new()) };
    let tolerance = ends_tolerance(faces);
    let (now, read_creased) = (creased(joins), creased(&read));
    let (mut creases, mut smooths) = (Vec::new(), Vec::new());
    for (i, sharp) in now.iter().enumerate() {
        let Some((a, b)) = seam_ends(faces, joins, i) else { continue };
        let Some(r) = seam_between(faces, &read, a, b, tolerance) else { continue };
        match (sharp, read_creased.get(r).copied().unwrap_or(*sharp)) {
            (true, false) => creases.push((a, b)),
            (false, true) => smooths.push((a, b)),
            _ => {}
        }
    }
    (creases, smooths)
}

/// The seams that run on from seam `seam` through the corners at its ends, `seam` first: at a
/// corner where four faces meet as a grid's do, the seam on the far side (the one that shares no
/// face with this one); the chain stops at a corner of other than four faces, at the network's
/// edge, and where it comes back round to its start.
pub fn crease_chain(faces: &[NurbsSurface], joins: &Joins, seam: usize) -> Vec<usize> {
    let Some(&first) = joins.seams.get(seam) else { return Vec::new() };
    let ends = |i: usize| -> Option<(usize, usize)> {
        let s = joins.seams.get(i)?;
        let ids = joins.side(faces, s.a, s.a_side);
        Some((*ids.first()?, *ids.last()?))
    };
    let regular: std::collections::HashSet<usize> = joins.corners.iter().filter(|c| !c.border && c.faces == 4).map(|c| c.point).collect();
    let mut chain = vec![seam];
    let Some((start_a, start_b)) = ends(seam) else { return chain };
    // Each way from the seam's two ends.
    for start in [start_a, start_b] {
        let (mut at, mut from, mut faces_of) = (start, seam, [first.a, first.b]);
        while regular.contains(&at) {
            let next: Vec<usize> = (0..joins.seams.len())
                .filter(|&t| t != from)
                .filter(|&t| ends(t).is_some_and(|(p, q)| p == at || q == at))
                .filter(|&t| {
                    let s = joins.seams[t];
                    !faces_of.contains(&s.a) && !faces_of.contains(&s.b)
                })
                .collect();
            let [t] = next.as_slice() else { break };
            if chain.contains(t) {
                break;
            }
            chain.push(*t);
            let Some((p, q)) = ends(*t) else { break };
            at = if p == at { q } else { p };
            from = *t;
            faces_of = [joins.seams[*t].a, joins.seams[*t].b];
        }
    }
    chain
}

/// What [`welded`] made: the faces, and the open sides it sewed into seams, each by its ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Welded {
    pub faces: Vec<NurbsSurface>,
    pub sewn: Vec<(Vec3, Vec3)>,
}

/// Network point `point` (a corner on the network's edge) put exactly at `onto` (another such
/// corner, or a place on an open side), as a corner dropped on another is: the two are one point
/// from then on, and two open sides of the same number of points whose ends now meet, either way
/// round, are sewn into one shared side, each pair of their points put at its middle. `None` for a
/// point that is not there or a place that is not one.
pub fn welded(faces: &[NurbsSurface], joins: &Joins, point: usize, onto: Vec3) -> Option<Welded> {
    if point >= joins.len() || !onto.is_finite() {
        return None;
    }
    let mut places = network_points(faces, joins);
    places[point] = onto;
    let mut faces = with_network_points(faces, joins, &places)?;
    let tolerance = ends_tolerance(&faces);
    let mut sewn = Vec::new();
    // Sew one pair at a time, reading the network again: a sewn pair changes which sides are open.
    for _ in 0..8 {
        let j = self::joins(&faces).ok()?;
        let places = network_points(&faces, &j);
        let ends = |&(f, s): &(usize, Side)| -> Option<(Vec<usize>, Vec3, Vec3)> {
            let ids = j.side(&faces, f, s);
            let (a, b) = (*places.get(*ids.first()?)?, *places.get(*ids.last()?)?);
            Some((ids, a, b))
        };
        let mut pair = None;
        'search: for (n, x) in j.open.iter().enumerate() {
            let Some((ix, xa, xb)) = ends(x) else { continue };
            for y in &j.open[n + 1..] {
                let Some((iy, ya, yb)) = ends(y) else { continue };
                if ix.len() != iy.len() || xa.dist(xb) <= tolerance {
                    continue;
                }
                if xa.dist(ya) <= tolerance && xb.dist(yb) <= tolerance {
                    pair = Some((ix, iy, false));
                    break 'search;
                }
                if xa.dist(yb) <= tolerance && xb.dist(ya) <= tolerance {
                    pair = Some((ix, iy, true));
                    break 'search;
                }
            }
        }
        let Some((ix, mut iy, reversed)) = pair else { break };
        if reversed {
            iy.reverse();
        }
        let mut moved = places.clone();
        for (&p, &q) in ix.iter().zip(&iy) {
            let middle = (places[p] + places[q]) * 0.5;
            moved[p] = middle;
            moved[q] = middle;
        }
        let (Some(&a), Some(&b)) = (ix.first(), ix.last()) else { break };
        sewn.push((moved[a], moved[b]));
        faces = with_network_points(&faces, &j, &moved)?;
    }
    Some(Welded { faces, sewn })
}

/// The way `s` faces on the whole: its normals across it, added up.
fn mean_normal(s: &NurbsSurface) -> Option<Vec3> {
    let net = points(s);
    let mut sum = Vec3::ZERO;
    for j in 0..=4 {
        for i in 0..=4 {
            if let Some((_, su, sv)) = frame(s, &net, f64::from(i) / 4.0, f64::from(j) / 4.0)
                && let Some(n) = su.cross(sv).normalized()
            {
                sum += n;
            }
        }
    }
    sum.normalized()
}

/// The way `s` faces at the middle of `side`.
fn side_normal(s: &NurbsSurface, side: Side) -> Option<Vec3> {
    let net = points(s);
    let [[u0, u1], [v0, v1]] = s.domain();
    let (u, v) = match side {
        Side::FirstRow => (0.5 * (u0 + u1), v0),
        Side::LastRow => (0.5 * (u0 + u1), v1),
        Side::FirstColumn => (u0, 0.5 * (v0 + v1)),
        Side::LastColumn => (u1, 0.5 * (v0 + v1)),
    };
    let (_, su, sv) = frame(s, &net, u, v)?;
    su.cross(sv).normalized()
}

/// How far from tangent two faces meet at most across the seams where they are held tangent (the
/// network's smooth ones), degrees: the angle between the faces' normals, sampled along each seam.
/// A crease (a seam where the faces met sharply) is not counted.
pub fn worst_kink(faces: &[NurbsSurface], joins: &Joins) -> f64 {
    let mut worst = 0.0f64;
    let held: std::collections::HashSet<usize> = joins.smooth.iter().map(|t| t.seam).collect();
    for seam in joins.seams.iter().enumerate().filter(|(i, _)| held.contains(i)).map(|(_, s)| s) {
        for k in 1..8 {
            let t = k as f64 / 8.0;
            let normal = |f: usize, side: Side, t: f64| -> Option<Vec3> {
                let s = &faces[f];
                let net = points(s);
                let [[u0, u1], [v0, v1]] = s.domain();
                let (u, v) = match side {
                    Side::FirstRow => (u0 + (u1 - u0) * t, v0),
                    Side::LastRow => (u0 + (u1 - u0) * t, v1),
                    Side::FirstColumn => (u0, v0 + (v1 - v0) * t),
                    Side::LastColumn => (u1, v0 + (v1 - v0) * t),
                };
                let (_, su, sv) = frame(s, &net, u, v)?;
                su.cross(sv).normalized()
            };
            let tb = if seam.reversed { 1.0 - t } else { t };
            if let (Some(a), Some(b)) = (normal(seam.a, seam.a_side, t), normal(seam.b, seam.b_side, tb)) {
                worst = worst.max(a.dot(b).abs().clamp(-1.0, 1.0).acos().to_degrees());
            }
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::Projector;
    use crate::{Scan, surface_edit};

    /// A grid mesh of `z = f(x, y)` over 120 × 60, `n` squares a side.
    fn field(n: u32, f: impl Fn(f64, f64) -> f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let mut positions = Vec::new();
        for j in 0..=n {
            for i in 0..=2 * n {
                let (x, y) = (120.0 * f64::from(i) / f64::from(2 * n), 60.0 * f64::from(j) / f64::from(n));
                positions.push(Vec3::new(x, y, f(x, y)));
            }
        }
        let w = 2 * n + 1;
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..2 * n {
                let k = j * w + i;
                triangles.push([k, k + 1, k + w + 1]);
                triangles.push([k, k + w + 1, k + w]);
            }
        }
        (positions, triangles)
    }

    fn dome(x: f64, y: f64) -> f64 {
        10.0 - 0.002 * ((x - 60.0).powi(2) + 2.0 * (y - 30.0).powi(2))
    }

    fn snapper(on: &Projector) -> impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>> + '_ {
        |ps: &[Vec3], along: Option<Vec3>| {
            ps.iter().map(|p| along.and_then(|d| on.along(*p, d)).map(|(q, _)| q).or_else(|| on.nearest(*p).map(|(q, _)| q))).collect()
        }
    }

    /// The first face of a network: a Free Form over four corners, fitted to the dome.
    fn first_face(on: &Projector) -> NurbsSurface {
        let corners = [Vec3::new(10.0, 10.0, 0.0), Vec3::new(50.0, 10.0, 0.0), Vec3::new(50.0, 50.0, 0.0), Vec3::new(10.0, 50.0, 0.0)];
        crate::free_form::fitted(corners, &crate::free_form::Fitting { count: 5, ..Default::default() }, snapper(on)).unwrap().surface
    }

    /// The colouring under a face paints the scan's triangles under it, each lifted a sliver off
    /// the scan towards its outside (each point once, so neighbours stay joined), lit by the scan's
    /// way out and coloured by a point near it; with the points thinned the same triangles are
    /// still painted.
    #[test]
    fn the_colouring_paints_the_scans_triangles_under_the_face() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let face = first_face(&on);
        let full = shade(&scan, std::slice::from_ref(&face), 2.0, 1_000_000);
        // The face covers 40 × 40 of the 120 × 60 field: 2/9 of its 14 400 triangles.
        let painted = full.paint.len();
        assert!((3000..=3600).contains(&painted), "{painted} triangles painted");
        for t in &full.paint {
            let [a, b, c] = t.corners.map(|[x, y, z]| Vec3::new(f64::from(x), f64::from(y), f64::from(z)));
            let m = (a + b + c) / 3.0;
            assert!((8.0..=52.0).contains(&m.x) && (8.0..=52.0).contains(&m.y), "under the face: {m:?}");
            let lift = m.z - dome(m.x, m.y);
            assert!(lift > 0.0 && lift < 0.1, "a sliver above the scan: {lift}");
            assert!(full.points.get(t.point as usize).is_some_and(|p| p.at.dist(m) < 3.0), "coloured by a point near it");
            let up = Vec3::new(f64::from(t.up[0]), f64::from(t.up[1]), f64::from(t.up[2]));
            // The dome's own normal there: (-dz/dx, -dz/dy, 1).
            let normal = Vec3::new(0.004 * (m.x - 60.0), 0.008 * (m.y - 30.0), 1.0).normalized().unwrap();
            assert!(up.dot(normal) > 0.999, "lit by the scan's way out: {up:?} vs {normal:?}");
        }
        // Each point nudged once: neighbours share their lifted corners (nudged each along its own
        // normal, no two corners on the dome were the same and the scan showed through the cracks).
        let corners: std::collections::HashSet<[u32; 3]> = full.paint.iter().flat_map(|t| t.corners.map(|c| c.map(f32::to_bits))).collect();
        assert!(corners.len() < painted, "{} different corners for {painted} triangles", corners.len());
        let thin = shade(&scan, std::slice::from_ref(&face), 2.0, 200);
        assert!(thin.points.len() <= 200);
        assert!(
            thin.paint.len().abs_diff(painted) * 6 < painted,
            "about the same triangles from 200 points (a rim half their spacing wide): {} vs {painted}",
            thin.paint.len()
        );
    }

    /// One surface cut at a knot is two faces of one network: they share the side, meet smoothly,
    /// and their corners are a grid's.
    #[test]
    fn two_halves_of_one_surface_are_a_network_of_one_smooth_seam() {
        let net: Vec<Vec3> =
            (0..6).flat_map(|j| (0..7).map(move |i| Vec3::new(f64::from(i) * 10.0, f64::from(j) * 10.0, f64::from(i * j) * 0.3))).collect();
        let whole = NurbsSurface::new([3, 3], [clamped_knots(7), clamped_knots(6)], &net, &[1.0; 42]).unwrap();
        let (a, b) = crate::merge::halves(&whole, 0).unwrap();
        let j = joins(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(j.seams.len(), 1, "{:?}", j.seams);
        assert_eq!(j.open.len(), 6);
        let side = a.count()[1];
        assert_eq!(j.len(), points(&a).len() + points(&b).len() - side, "the shared side's points are one");
        assert!(j.smooth.len() >= side - 2, "held tangent along it: {}", j.smooth.len());
        assert!(j.irregular().next().is_none());
        assert!(worst_kink(&[a, b], &j) < 1e-6);
    }

    /// A face added on an open edge of a face fitted to a dome lies on the dome, shares the edge
    /// exactly, and meets the first face tangentially.
    #[test]
    fn a_face_added_on_an_edge_lies_on_the_scan_and_meets_the_first_smoothly() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let first = first_face(&on);
        let faces = vec![first];
        let j = joins(&faces).unwrap();
        // The first face's u = 1 side runs along x = 50: grow out to x = 90.
        let side = *j.open.iter().find(|(_, s)| *s == Side::LastColumn).unwrap();
        let grown = with_face(&faces, &j, side.0, side.1, [Vec3::new(90.0, 10.0, 0.0), Vec3::new(90.0, 50.0, 0.0)], 5, snapper(&on)).unwrap();
        assert_eq!(grown.len(), 2);
        let gj = joins(&grown).unwrap();
        assert_eq!(gj.seams.len(), 1, "the new face shares the edge");
        let under = surface_edit::covered(&scan, &grown[1], 2.0, 5_000);
        let (rms, max) = surface_edit::distance(&grown[1], &under);
        assert!(rms < 0.05 && max < 0.25, "on the dome: {rms} {max}");
        let kink = worst_kink(&grown, &gj);
        assert!(kink < 0.5, "tangent across the edge: {kink}°");
        let check = crate::surface_check::check_whole(&grown[1], 24).unwrap();
        eprintln!("new face: least angle {} stretch {}", check.least_angle, check.stretch);
        // The first face moved only next to the edge, and stays on the dome.
        let moved_points = points(&grown[0]).iter().zip(points(&faces[0])).filter(|(a, b)| a.dist(*b) > 1e-9).count();
        assert!(moved_points <= 5, "only the row next to the edge: {moved_points}");
        let under = surface_edit::covered(&scan, &grown[0], 2.0, 5_000);
        let (rms, _) = surface_edit::distance(&grown[0], &under);
        assert!(rms < 0.05, "the first face still on the dome: {rms}");
    }

    /// A dot dragged with its neighbours held: the surface runs through the new place and through
    /// every other dot, the seam stays one and tangent.
    #[test]
    fn a_dot_dragged_takes_the_surface_through_it_and_holds_the_rest() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let faces = vec![first_face(&on)];
        let j = joins(&faces).unwrap();
        let side = *j.open.iter().find(|(_, s)| *s == Side::LastColumn).unwrap();
        let grown = with_face(&faces, &j, side.0, side.1, [Vec3::new(90.0, 10.0, 0.0), Vec3::new(90.0, 50.0, 0.0)], 5, snapper(&on)).unwrap();
        let gj = joins(&grown).unwrap();
        let before = dots(&grown, &gj);
        // A dot on the seam, not at a corner, pulled up 3 mm.
        let seam_dot = (0..gj.len()).find(|p| gj.on_seam(*p) && gj.corners.iter().all(|c| c.point != *p)).unwrap();
        let to = before[seam_dot].at + Vec3::new(0.0, 0.0, 3.0);
        let after = dot_moved(&grown, &gj, seam_dot, to, DOT_RING, true).unwrap();
        let aj = joins(&after).unwrap();
        assert_eq!(aj.seams.len(), 1, "still one seam");
        let now = dots(&after, &aj);
        assert!(now[seam_dot].at.dist(to) < 0.05, "the surface runs through where the dot went: {}", now[seam_dot].at.dist(to));
        // The points next to it across the seam rise with it to keep the faces tangent (a seam is
        // held smooth: the drag makes a bump, not a spike); those beyond the ring it moves stay.
        let near = region(&grown, &gj, &[seam_dot], DOT_RING - 1);
        let worst = (0..gj.len()).filter(|p| !near[*p]).map(|p| now[p].at.dist(before[p].at)).fold(0.0, f64::max);
        assert!(worst < 0.25, "the dots beyond the ring held: {worst}");
        let kink = worst_kink(&after, &aj);
        assert!(kink < 1.0, "still tangent: {kink}°");
    }

    /// Three faces round a corner on the network's edge (an L): the corner where they meet is
    /// irregular, and the open sides are the L's outline.
    #[test]
    fn three_faces_round_a_corner_make_it_irregular() {
        let square = |x: f64, y: f64| -> NurbsSurface {
            let net: Vec<Vec3> = (0..4).flat_map(|j| (0..4).map(move |i| Vec3::new(x + 10.0 * f64::from(i), y + 10.0 * f64::from(j), 0.0))).collect();
            NurbsSurface::new([3, 3], [clamped_knots(4), clamped_knots(4)], &net, &[1.0; 16]).unwrap()
        };
        let faces = [square(0.0, 0.0), square(30.0, 0.0), square(0.0, 30.0)];
        let j = joins(&faces).unwrap();
        assert_eq!(j.seams.len(), 2);
        let odd: Vec<&Corner> = j.irregular().collect();
        assert!(odd.is_empty(), "an L of three squares has no corner of three: {odd:?}");
        // A fourth square closing the L into a square makes the middle a grid's corner.
        let four = [faces[0].clone(), faces[1].clone(), faces[2].clone(), square(30.0, 30.0)];
        let j4 = joins(&four).unwrap();
        assert_eq!(j4.seams.len(), 4);
        let middle = j4.corners.iter().find(|c| c.faces == 4).unwrap();
        assert!(!middle.border && !middle.irregular());
    }

    /// A closed box scanned, `size` a side, each side an `n` × `n` grid.
    fn box_scan(size: f64, n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (mut positions, mut triangles) = (Vec::new(), Vec::new());
        // Each side: an origin and two directions spanning it, wound outward.
        let sides = [
            (Vec3::new(0.0, 0.0, 0.0), Vec3::new(size, 0.0, 0.0), Vec3::new(0.0, 0.0, size)),
            (Vec3::new(0.0, size, 0.0), Vec3::new(0.0, 0.0, size), Vec3::new(size, 0.0, 0.0)),
            (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, size, 0.0), Vec3::new(size, 0.0, 0.0)),
            (Vec3::new(0.0, 0.0, size), Vec3::new(size, 0.0, 0.0), Vec3::new(0.0, size, 0.0)),
            (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, size), Vec3::new(0.0, size, 0.0)),
            (Vec3::new(size, 0.0, 0.0), Vec3::new(0.0, size, 0.0), Vec3::new(0.0, 0.0, size)),
        ];
        for (o, du, dv) in sides {
            let start = positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    positions.push(o + du * (f64::from(i) / f64::from(n)) + dv * (f64::from(j) / f64::from(n)));
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let k = start + j * (n + 1) + i;
                    triangles.push([k, k + 1, k + n + 2]);
                    triangles.push([k, k + n + 2, k + n + 1]);
                }
            }
        }
        (positions, triangles)
    }

    /// Faces built round the corner of a scanned box, the third sharing two sides with the first
    /// two (as a network closes round a part): each lies on its side of the box, the seams where the
    /// box turns are left creases (not pulled tangent, which folded the third face), and no corner is
    /// flat.
    #[test]
    fn faces_round_the_corner_of_a_box_meet_at_creases_and_the_third_shares_two_sides() {
        let (positions, triangles) = box_scan(40.0, 40);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let corners = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 40.0), Vec3::new(0.0, 0.0, 40.0)];
        let front = crate::free_form::fitted(corners, &crate::free_form::Fitting { count: 5, ..Default::default() }, snapper(&on)).unwrap().surface;
        let faces = vec![front];
        let j = joins(&faces).unwrap();
        // The bottom, on the front's edge along z = 0.
        let ((_, bottom_side), _) = open_side_near(&faces, &j, Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0)).unwrap();
        let two = with_face(&faces, &j, 0, bottom_side, [Vec3::new(0.0, 40.0, 0.0), Vec3::new(40.0, 40.0, 0.0)], 5, snapper(&on)).unwrap();
        let j2 = joins(&two).unwrap();
        assert_eq!(j2.seams.len(), 1);
        assert!((seam_angle(&two, &j2.seams[0]) - 90.0).abs() < 5.0, "the box's edge kept: {}", seam_angle(&two, &j2.seams[0]));
        assert!(j2.smooth.is_empty(), "a crease, not held tangent");
        // The +x side: built on the bottom's edge along x = 40, its side up from (40, 0, 0) the front's.
        let ((f, side), _) = open_side_near(&two, &j2, Vec3::new(40.0, 0.0, 0.0), Vec3::new(40.0, 40.0, 0.0)).unwrap();
        let three = with_face(&two, &j2, f, side, [Vec3::new(40.0, 0.0, 40.0), Vec3::new(40.0, 40.0, 40.0)], 5, snapper(&on)).unwrap();
        let j3 = joins(&three).unwrap();
        assert_eq!(j3.seams.len(), 3, "the third face shares a side with each of the others");
        for seam in &j3.seams {
            assert!((seam_angle(&three, seam) - 90.0).abs() < 5.0, "{seam:?}: {}", seam_angle(&three, seam));
        }
        for face in &three {
            assert!(flattest_corner(face) > 45.0, "no flat corner: {}", flattest_corner(face));
            let under = surface_edit::covered(&scan, face, 2.0, 5_000);
            let (rms, _) = surface_edit::distance(face, &under);
            assert!(rms < 0.05, "on its side of the box: {rms}");
        }
        assert_eq!(j3.irregular().count(), 1, "the box's corner, where three faces meet");

        // On round the box: the back, the -x side (sharing the front's and the back's sides) and the
        // top (sharing all four of its sides): the network closes, every corner one of three faces.
        let mut all = three;
        for (edge, far) in [
            ([Vec3::new(0.0, 40.0, 0.0), Vec3::new(40.0, 40.0, 0.0)], [Vec3::new(0.0, 40.0, 40.0), Vec3::new(40.0, 40.0, 40.0)]),
            ([Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 40.0, 0.0)], [Vec3::new(0.0, 0.0, 40.0), Vec3::new(0.0, 40.0, 40.0)]),
            ([Vec3::new(0.0, 0.0, 40.0), Vec3::new(40.0, 0.0, 40.0)], [Vec3::new(0.0, 40.0, 40.0), Vec3::new(40.0, 40.0, 40.0)]),
        ] {
            let jn = joins(&all).unwrap();
            let ((f, side), _) = open_side_near(&all, &jn, edge[0], edge[1]).unwrap();
            all = with_face(&all, &jn, f, side, far, 5, snapper(&on)).unwrap();
        }
        let closed = joins(&all).unwrap();
        assert_eq!((all.len(), closed.seams.len(), closed.open.len()), (6, 12, 0), "closed round the box");
        assert_eq!(closed.irregular().count(), 8, "each of the box's corners");
        assert!(connected(&all, &closed));
        for face in &all {
            let under = surface_edit::covered(&scan, face, 2.0, 5_000);
            assert!(surface_edit::distance(face, &under).0 < 0.05);
            assert!(flattest_corner(face) > 45.0);
        }
    }

    /// A sphere scanned, `radius` round the origin: a box's sides, `n` × `n` each, pushed out onto it.
    fn sphere_scan(radius: f64, n: u32) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let (positions, triangles) = box_scan(2.0, n);
        let middle = Vec3::new(1.0, 1.0, 1.0);
        let on = positions.iter().map(|p| (*p - middle).normalized().map_or(Vec3::ZERO, |d| d * radius)).collect();
        (on, triangles)
    }

    /// Faces built round a sphere as round a box (the cube's six, each corner one of three faces):
    /// the seams are smooth, so each is held tangent, yet a corner on the network's edge whose sides
    /// turn is not held in a line before its faces are all there, and each face's free sides go onto the sphere
    /// square to it (great circles, not a plane's cut, which runs on through the corner): the third
    /// face round a corner closes it at the sphere's own angle instead of a flat one, and the
    /// network closes on the sphere.
    #[test]
    fn faces_round_a_sphere_close_with_three_at_each_corner() {
        let radius = 40.0;
        let (positions, triangles) = sphere_scan(radius, 40);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let c = radius / 3.0f64.sqrt();
        let p = |x: f64, y: f64, z: f64| Vec3::new(x * c, y * c, z * c);
        // The top over four great circles' arcs, as a cube's layout on the sphere draws them.
        let arc = |a: Vec3, b: Vec3| -> Vec<Vec3> {
            (0..=24).filter_map(|k| (a + (b - a) * (f64::from(k) / 24.0)).normalized().map(|d| d * radius)).collect()
        };
        let corners = [p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)];
        let sides: Vec<Vec<Vec3>> = (0..4).map(|k| arc(corners[k], corners[(k + 1) % 4])).collect();
        let boundary = crate::boundary_face::order(&sides, 0.0).unwrap();
        let flat = crate::boundary_face::face(&boundary, &[], [8, 8]).unwrap();
        let under = surface_edit::covered(&scan, &flat, 10.0, 5_000);
        let top = crate::boundary_face::face(&boundary, &under, [8, 8]).unwrap();
        let mut all = vec![top];
        // The four round the top, then the bottom (sharing all four of its sides).
        for (edge, far) in [
            ([p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0)], [p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0)]),
            ([p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0)], [p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0)]),
            ([p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)], [p(1.0, 1.0, -1.0), p(-1.0, 1.0, -1.0)]),
            ([p(-1.0, 1.0, 1.0), p(-1.0, -1.0, 1.0)], [p(-1.0, 1.0, -1.0), p(-1.0, -1.0, -1.0)]),
            ([p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0)], [p(-1.0, 1.0, -1.0), p(1.0, 1.0, -1.0)]),
        ] {
            let jn = joins(&all).unwrap();
            let ((f, side), _) = open_side_near(&all, &jn, edge[0], edge[1]).unwrap();
            all = with_face(&all, &jn, f, side, far, 8, snapper(&on)).unwrap_or_else(|e| panic!("face {} refused: {e}", all.len()));
        }
        let closed = joins(&all).unwrap();
        assert_eq!((all.len(), closed.seams.len(), closed.open.len()), (6, 12, 0), "closed round the sphere");
        assert_eq!(closed.irregular().count(), 8, "a corner of three faces at each of the cube's");
        assert_eq!(creased(&closed).iter().filter(|c| **c).count(), 0, "every seam smooth");
        for face in &all {
            let under = surface_edit::covered(&scan, face, 2.0, 5_000);
            let (rms, max) = surface_edit::distance(face, &under);
            assert!(rms < 0.1 && max < 0.5, "on the sphere: {rms} {max}");
            assert!(flattest_corner(face) > 45.0, "no flat corner: {}", flattest_corner(face));
        }
        let kink = worst_kink(&all, &closed);
        assert!(kink < 1.0, "tangent along the seams: {kink}°");
    }

    /// A cup of five faces round a sphere (the top and the four round it) thickened: the network
    /// moved along its normals is the sphere grown by the thickness, its faces' sides shared as
    /// the cup's are, and a strip along each open side joins the two exactly, so every side of
    /// every face is shared: the faces close into one solid. A closed network is refused.
    #[test]
    fn a_cup_of_faces_thickens_into_faces_that_close() {
        let radius = 40.0;
        let (positions, triangles) = sphere_scan(radius, 40);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let c = radius / 3.0f64.sqrt();
        let p = |x: f64, y: f64, z: f64| Vec3::new(x * c, y * c, z * c);
        let arc = |a: Vec3, b: Vec3| -> Vec<Vec3> {
            (0..=24).filter_map(|k| (a + (b - a) * (f64::from(k) / 24.0)).normalized().map(|d| d * radius)).collect()
        };
        let corners = [p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)];
        let sides: Vec<Vec<Vec3>> = (0..4).map(|k| arc(corners[k], corners[(k + 1) % 4])).collect();
        let boundary = crate::boundary_face::order(&sides, 0.0).unwrap();
        let flat = crate::boundary_face::face(&boundary, &[], [8, 8]).unwrap();
        let under = surface_edit::covered(&scan, &flat, 10.0, 5_000);
        let mut cup = vec![crate::boundary_face::face(&boundary, &under, [8, 8]).unwrap()];
        for (edge, far) in [
            ([p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0)], [p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0)]),
            ([p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0)], [p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0)]),
            ([p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0)], [p(1.0, 1.0, -1.0), p(-1.0, 1.0, -1.0)]),
            ([p(-1.0, 1.0, 1.0), p(-1.0, -1.0, 1.0)], [p(-1.0, 1.0, -1.0), p(-1.0, -1.0, -1.0)]),
        ] {
            let jn = joins(&cup).unwrap();
            let ((f, side), _) = open_side_near(&cup, &jn, edge[0], edge[1]).unwrap();
            cup = with_face(&cup, &jn, f, side, far, 8, snapper(&on)).unwrap();
        }
        let cj = joins(&cup).unwrap();
        assert_eq!((cup.len(), cj.seams.len(), cj.open.len()), (5, 8, 4));
        let way = orientations(&cup, &cj);
        // Which way the first face faces: out of the sphere or into it.
        let (pt, su, sv) = frame(&cup[0], &points(&cup[0]), 0.5, 0.5).unwrap();
        let out = if su.cross(sv).dot(pt) > 0.0 { 1.0 } else { -1.0 };
        let t = 3.0;
        let solid = thickened(&cup, 0.0, t * out).unwrap();
        assert_eq!(solid.len(), 5 + 5 + 4, "the cup, its offset and a strip on each open side");
        let sj = joins(&solid).unwrap();
        assert_eq!(sj.open.len(), 0, "every side shared: the faces close");
        assert_eq!(sj.seams.len(), 8 + 8 + 4 * 2 + 4);
        // Each offset face runs the thickness off its cup face, along the cup face's normal (turned
        // as the first face's), at the same places.
        let mut worst = 0.0f64;
        for k in 0..5 {
            let (face, moved) = (&cup[k], &solid[5 + k]);
            for i in 0..=20 {
                for j in 0..=20 {
                    let (u, v) = (f64::from(i) / 20.0, f64::from(j) / 20.0);
                    let (p0, su, sv) = frame(face, &points(face), u, v).unwrap();
                    let (q, _, _) = frame(moved, &points(moved), u, v).unwrap();
                    let n = su.cross(sv).normalized().unwrap() * way[k] * out;
                    worst = worst.max(q.dist(p0 + n * t));
                }
            }
        }
        // Worst at the corners of three faces, where the cup's faces' normals differ a little and
        // the one point the offset faces share there cannot be off each of them by the thickness.
        assert!(worst < 0.15, "the offset {t} mm off the cup everywhere: {worst}");
        let kink = worst_kink(&solid[5..10], &joins(&solid[5..10]).unwrap());
        assert!(kink < 1.0, "the offset as smooth as the cup: {kink}°");
        // Symmetric: both moved, by half each way.
        let both = thickened(&cup, -1.5 * out, 1.5 * out).unwrap();
        let (q, _, _) = frame(&both[0], &points(&both[0]), 0.5, 0.5).unwrap();
        assert!((q.dist(Vec3::ZERO) - (radius - 1.5)).abs() < 0.05, "{}", q.dist(Vec3::ZERO));
        // Refused: a closed network, no thickness, and hostile values.
        let jn = joins(&cup).unwrap();
        let ((f, side), _) = open_side_near(&cup, &jn, p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0)).unwrap();
        let closed = with_face(&cup, &jn, f, side, [p(-1.0, 1.0, -1.0), p(1.0, 1.0, -1.0)], 8, snapper(&on)).unwrap();
        assert_eq!(thickened(&closed, 0.0, 3.0), Err(NetworkError::Closed));
        assert_eq!(thickened(&cup, 1.0, 1.0), Err(NetworkError::Failed));
        assert_eq!(thickened(&cup, 0.0, f64::NAN), Err(NetworkError::Failed));
    }

    /// A face over a rim that runs on smoothly through its corners would have flat corners: refused in
    /// words before the kernel sees it.
    #[test]
    fn a_face_with_a_flat_corner_is_refused() {
        // Two faces side by side in one plane, their outer sides in a straight line: a face built on
        // the first's far side out to the second's far corner would bend back flat at the shared one.
        let square = |x: f64| -> NurbsSurface {
            let net: Vec<Vec3> = (0..4).flat_map(|j| (0..4).map(move |i| Vec3::new(x + 10.0 * f64::from(i), 10.0 * f64::from(j), 0.0))).collect();
            NurbsSurface::new([3, 3], [clamped_knots(4), clamped_knots(4)], &net, &[1.0; 16]).unwrap()
        };
        let flat = NurbsSurface::new(
            [3, 3],
            [clamped_knots(4), clamped_knots(4)],
            &(0..4)
                .flat_map(|j| (0..4).map(move |i| Vec3::new(10.0 * f64::from(i), 10.0 * f64::from(j) * f64::from(i) / 3.0, 0.0)))
                .collect::<Vec<_>>(),
            &[1.0; 16],
        )
        .unwrap();
        assert!(flattest_corner(&flat) < FLAT_DEGREES, "a corner where two sides meet in a line: {}", flattest_corner(&flat));
        assert!(flattest_corner(&square(0.0)) > 89.0);
    }

    /// A face taken away leaves the rest as they were; the last face, and one between two others,
    /// stay. Smoothing a wavy face straightens it, its corners where they were.
    #[test]
    fn a_face_is_taken_away_and_a_wavy_net_smoothed() {
        let square = |x: f64, wave: f64| -> NurbsSurface {
            let net: Vec<Vec3> = (0..5)
                .flat_map(|j| (0..5).map(move |i| Vec3::new(x + 10.0 * f64::from(i), 10.0 * f64::from(j), if (i + j) % 2 == 1 { wave } else { 0.0 })))
                .collect();
            NurbsSurface::new([3, 3], [clamped_knots(5), clamped_knots(5)], &net, &[1.0; 25]).unwrap()
        };
        let three = [square(0.0, 0.0), square(40.0, 0.0), square(80.0, 0.0)];
        assert_eq!(without_face(&three, 1), Err(NetworkError::WouldSplit));
        let two = without_face(&three, 2).unwrap();
        assert_eq!(two, three[..2].to_vec());
        assert_eq!(without_face(&two[..1], 0), Err(NetworkError::LastFace));

        let wavy = [square(0.0, 1.0)];
        let j = joins(&wavy).unwrap();
        let bumps = |f: &NurbsSurface| points(f).iter().map(|p| p.z.abs()).fold(0.0, f64::max);
        let mut smooth = wavy.to_vec();
        for _ in 0..5 {
            smooth = smoothed(&smooth, &j, &[], 1.0).unwrap();
        }
        assert!(bumps(&smooth[0]) < 0.5 * bumps(&wavy[0]), "{} against {}", bumps(&smooth[0]), bumps(&wavy[0]));
        for k in [0, 4, 20, 24] {
            assert_eq!(points(&smooth[0])[k], points(&wavy[0])[k], "corner {k} held");
        }
        assert!(smoothed(&wavy, &j, &[], f64::NAN).is_none());
    }

    /// Hostile input is refused or left alone, never a panic.
    #[test]
    fn hostile_input_is_refused() {
        let bad = NurbsSurface::new([1, 1], [vec![0.0, 0.0, 1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]], &[Vec3::ZERO; 4], &[1.0; 4]).unwrap();
        assert_eq!(joins(&[bad]), Err(NetworkError::NotCubic));
        assert!(joins(&[]).is_ok_and(|j| j.is_empty()));
        let net: Vec<Vec3> = (0..4).flat_map(|j| (0..4).map(move |i| Vec3::new(f64::from(i), f64::from(j), 0.0))).collect();
        let s = NurbsSurface::new([3, 3], [clamped_knots(4), clamped_knots(4)], &net, &[1.0; 16]).unwrap();
        let j = joins(std::slice::from_ref(&s)).unwrap();
        let none = |ps: &[Vec3], _: Option<Vec3>| vec![None; ps.len()];
        assert!(with_face(std::slice::from_ref(&s), &j, 0, Side::FirstRow, [Vec3::new(f64::NAN, 0.0, 0.0), Vec3::ZERO], 4, none).is_err());
        assert!(with_face(std::slice::from_ref(&s), &j, 5, Side::FirstRow, [Vec3::ZERO, Vec3::ZERO], 4, none).is_err());
        assert!(dot_moved(std::slice::from_ref(&s), &j, 99, Vec3::ZERO, 2, true).is_none(), "no such dot: nothing moves");
        assert!(moved(&[s], &j, &Moves { free: &[], goals: &[], tangent: 1.0, even: 1.0, bend: 0.0, also_smooth: &[] }).is_none());
    }
    /// Two faces fitted over a ridge the smooth way (its sides meet within 25°, so Add Face held
    /// them tangent, rounding the ridge): creasing their seam lets the tangency go and refits the
    /// rows next to it, so the faces take the ridge; made smooth again, with the scan or without,
    /// they meet tangentially again. A seam named by its ends or by a point near it is found.
    #[test]
    fn a_creased_seam_follows_a_ridge_and_made_smooth_again_meets_tangentially() {
        let ridge = |x: f64, _y: f64| 10.0 - 0.2 * (x - 60.0).abs();
        let (positions, triangles) = field(60, ridge);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        // Corners on the scan (a corner below the ridge would snap to the slope beside it).
        let corners = [Vec3::new(20.0, 10.0, 2.0), Vec3::new(60.0, 10.0, 10.0), Vec3::new(60.0, 50.0, 10.0), Vec3::new(20.0, 50.0, 2.0)];
        let first = crate::free_form::fitted(corners, &crate::free_form::Fitting { count: 5, ..Default::default() }, snapper(&on)).unwrap().surface;
        let faces = vec![first];
        let j = joins(&faces).unwrap();
        let side = *j.open.iter().find(|(_, s)| *s == Side::LastColumn).unwrap();
        let grown = with_face(&faces, &j, side.0, side.1, [Vec3::new(100.0, 10.0, 2.0), Vec3::new(100.0, 50.0, 2.0)], 5, snapper(&on)).unwrap();
        let gj = joins(&grown).unwrap();
        assert_eq!(creased(&gj), vec![false], "held tangent across the ridge");
        let rms = |faces: &[NurbsSurface]| -> f64 {
            let (mut squares, mut n) = (0.0, 0usize);
            for f in faces {
                let under = surface_edit::covered(&scan, f, 2.0, 5_000);
                let (r, _) = surface_edit::distance(f, &under);
                squares += r * r * under.len() as f64;
                n += under.len();
            }
            (squares / n.max(1) as f64).sqrt()
        };
        let before = rms(&grown);
        assert!(before > 0.05, "the ridge rounded over: {before}");
        // Found by a point near it, and by its ends either way round.
        let (near, off) = seam_near(&grown, &gj, Vec3::new(60.0, 30.0, 10.0)).unwrap();
        assert!(near == 0 && off < 1.0, "{near} {off}");
        let (a, b) = seam_ends(&grown, &gj, 0).unwrap();
        assert!((a.x - 60.0).abs() < 1e-6 && (b.x - 60.0).abs() < 1e-6, "{a:?} {b:?}");
        assert_eq!(seam_between(&grown, &gj, b, a, 1.0), Some(0));
        assert_eq!(seam_between(&grown, &gj, Vec3::new(0.0, 0.0, 0.0), b, 1.0), None);
        // Creased: the tangency let go, the rows next to the seam refitted to the ridge.
        let held = vec![false; gj.len()];
        let sharp = without_tangency(&gj, 0);
        assert_eq!(creased(&sharp), vec![true]);
        let folded = refit_near_seam(&grown, &sharp, 0, &positions, 2.0, &held).unwrap();
        let angle = seam_angle(&folded, &gj.seams[0]);
        let after = rms(&folded);
        assert!(angle > 15.0 && after < before / 2.0, "{angle}° {after} against {before}");
        assert!(joins(&folded).unwrap().smooth.is_empty(), "read back as a crease");
        // Made smooth again, with the scan: tangent, the ridge rounded over once more.
        let smooth = with_tangency(&folded, &sharp, 0);
        assert!(!smooth.smooth.is_empty());
        let rounded = refit_near_seam(&folded, &smooth, 0, &positions, 2.0, &held).unwrap();
        assert!(worst_kink(&rounded, &smooth) < 1.0, "{}", worst_kink(&rounded, &smooth));
        // And without a scan: the faces change only as far as meeting tangentially needs.
        let eased = refit_near_seam(&folded, &smooth, 0, &[], 2.0, &held).unwrap();
        assert!(worst_kink(&eased, &smooth) < 1.0, "{}", worst_kink(&eased, &smooth));
        // Hostile: a seam that is not there, a point that is not a place, nothing free to move.
        assert!(across_seam(&grown, &gj, 7).is_empty());
        assert!(seam_near(&grown, &gj, Vec3::new(f64::NAN, 0.0, 0.0)).is_none());
        assert!(seam_between(&grown, &gj, Vec3::new(f64::NAN, 0.0, 0.0), b, 1.0).is_none());
        assert!(refit_near_seam(&grown, &sharp, 7, &positions, 2.0, &held).is_none());
        assert!(refit_near_seam(&grown, &sharp, 0, &positions, f64::NAN, &held).is_none());
        assert!(refit_near_seam(&grown, &sharp, 0, &positions, 2.0, &vec![true; gj.len()]).is_none());
        assert_eq!(without_tangency(&gj, 7), gj);
    }
    /// A dot pushed up off the dome is brushed back toward its neighbours: with the scan's points
    /// the surface stays on the dome, without them it is faired; the brush's rim hardly moves, and
    /// the network's own corners never do. A brush over nothing that may move does nothing.
    #[test]
    fn the_brush_evens_out_a_bump_and_keeps_the_points_on_the_scan() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let faces = vec![first_face(&on)];
        let j = joins(&faces).unwrap();
        let middle = 2 * 5 + 2;
        let start = dots(&faces, &j);
        let bumped = dot_moved(&faces, &j, middle, start[middle].at + Vec3::new(0.0, 0.0, 3.0), DOT_RING, true).unwrap();
        let height = |faces: &[NurbsSurface]| dots(faces, &j)[middle].at.z - dome(start[middle].at.x, start[middle].at.y);
        assert!(height(&bumped) > 2.9, "{}", height(&bumped));
        // Under the brush: the bump fully, less the further out (the corners least, and held anyway).
        let under: Vec<(usize, f64)> = (0..25usize).map(|p| (p, 1.0 - ((p % 5).abs_diff(2).max((p / 5).abs_diff(2))) as f64 / 3.0)).collect();
        let mut on_scan = bumped.clone();
        for _ in 0..6 {
            let goals = feet(&on_scan, &positions, 2.0, &[true]);
            on_scan = brushed(&on_scan, &j, &under, &goals, 0.5).unwrap();
        }
        assert!(height(&on_scan) < 1.0, "brushed down onto the dome: {}", height(&on_scan));
        let under_it = surface_edit::covered(&scan, &on_scan[0], 2.0, 5_000);
        assert!(surface_edit::distance(&on_scan[0], &under_it).0 < 0.2, "still on the scan");
        let mut faired = bumped.clone();
        for _ in 0..6 {
            faired = brushed(&faired, &j, &under, &[], 0.5).unwrap();
        }
        assert!(height(&faired) < 2.0, "faired without the scan: {}", height(&faired));
        let (before, after) = (dots(&bumped, &j), dots(&faired, &j));
        assert!(before[0].at.dist(after[0].at) < 1e-9, "the corner stays");
        assert!(before[middle + 1].at.dist(after[middle + 1].at) < before[middle].at.dist(after[middle].at), "the rim moves less than the middle");
        // Nothing under the brush that may move: the corner alone, a point that is not there, no strength.
        assert!(brushed(&bumped, &j, &[(0, 1.0)], &[], 0.5).is_none());
        assert!(brushed(&bumped, &j, &[(99, 1.0)], &[], 0.5).is_none());
        assert!(brushed(&bumped, &j, &under, &[], 0.0).is_none());
        assert!(brushed(&bumped, &j, &[(middle, f64::NAN)], &[], 0.5).is_none());
    }

    /// A flat square `x`..`x + 30` by `y`..`y + 30` at height `z`, 4 × 4 points.
    fn square_at(x: f64, y: f64, z: f64) -> NurbsSurface {
        let net: Vec<Vec3> = (0..4).flat_map(|j| (0..4).map(move |i| Vec3::new(x + 10.0 * f64::from(i), y + 10.0 * f64::from(j), z))).collect();
        NurbsSurface::new([3, 3], [clamped_knots(4), clamped_knots(4)], &net, &[1.0; 16]).unwrap()
    }

    /// In a grid of three by three squares the seam through the middle runs on across the grid's
    /// corners to the edge both ways; in an L of three squares it stops at the corner where three
    /// faces meet.
    #[test]
    fn a_crease_chain_runs_on_through_grid_corners() {
        let grid: Vec<NurbsSurface> = (0..3).flat_map(|j| (0..3).map(move |i| square_at(30.0 * f64::from(i), 30.0 * f64::from(j), 0.0))).collect();
        let j = joins(&grid).unwrap();
        assert_eq!(j.seams.len(), 12);
        let (middle, _) = seam_near(&grid, &j, Vec3::new(45.0, 30.0, 0.0)).unwrap();
        let chain = crease_chain(&grid, &j, middle);
        assert_eq!(chain.len(), 3, "the line y = 30 all the way across: {chain:?}");
        assert_eq!(chain[0], middle);
        for &s in &chain {
            let (a, b) = seam_ends(&grid, &j, s).unwrap();
            assert!((a.y - 30.0).abs() < 1e-9 && (b.y - 30.0).abs() < 1e-9, "{a:?} {b:?}");
        }
        let l = [square_at(0.0, 0.0, 0.0), square_at(30.0, 0.0, 0.0), square_at(0.0, 30.0, 0.0)];
        let jl = joins(&l).unwrap();
        let (s, _) = seam_near(&l, &jl, Vec3::new(30.0, 15.0, 0.0)).unwrap();
        assert_eq!(crease_chain(&l, &jl, s), vec![s], "no grid corner to run on through");
        assert!(crease_chain(&l, &jl, 99).is_empty());
    }

    /// Seams creased or held smooth by hand are kept by their ends: read again from the faces
    /// alone they are lost, read with what was kept they come back, and only they are kept.
    #[test]
    fn edges_held_by_hand_are_kept_by_their_ends() {
        let two = [square_at(0.0, 0.0, 0.0), square_at(30.0, 0.0, 0.0)];
        let j = joins(&two).unwrap();
        assert_eq!(creased(&j), vec![false], "flat: smooth");
        let sharp = without_tangency(&j, 0);
        let (creases, smooths) = held_by_hand(&two, &sharp);
        assert_eq!((creases.len(), smooths.len()), (1, 0));
        assert!(held_by_hand(&two, &j).0.is_empty(), "nothing by hand");
        let back = joins_keeping(&two, &creases, &smooths, ends_tolerance(&two)).unwrap();
        assert_eq!(creased(&back), vec![true]);
        // The ends either way round name it; ends elsewhere do not.
        let turned: Vec<(Vec3, Vec3)> = creases.iter().map(|(a, b)| (*b, *a)).collect();
        assert_eq!(creased(&joins_keeping(&two, &turned, &[], 1e-6).unwrap()), vec![true]);
        let elsewhere = [(Vec3::new(0.0, 0.0, 9.0), Vec3::new(1.0, 0.0, 9.0))];
        assert_eq!(creased(&joins_keeping(&two, &elsewhere, &[], 1e-6).unwrap()), vec![false]);
    }

    /// A corner dropped on another's is one point with it; dropping the second corner of the side
    /// sews the two open sides into one seam, each pair of points at its middle.
    #[test]
    fn corners_dropped_on_corners_weld_and_a_side_whose_ends_meet_is_sewn() {
        let faces = [square_at(0.0, 0.0, 0.0), square_at(31.0, 0.0, 2.0)];
        let j = joins(&faces).unwrap();
        assert!(j.seams.is_empty(), "apart");
        let places = network_points(&faces, &j);
        let at = |p: Vec3| (0..places.len()).min_by(|a, b| places[*a].dist(p).total_cmp(&places[*b].dist(p))).unwrap();
        let (a, b) = (at(Vec3::new(31.0, 0.0, 2.0)), at(Vec3::new(31.0, 30.0, 2.0)));
        let once = welded(&faces, &j, a, Vec3::new(30.0, 0.0, 0.0)).unwrap();
        assert!(once.sewn.is_empty(), "one corner: no side sewn yet");
        let j1 = joins(&once.faces).unwrap();
        assert_eq!(j1.len(), j.len() - 1, "the two corners are one point");
        let places1 = network_points(&once.faces, &j1);
        let b1 = (0..places1.len()).min_by(|x, y| places1[*x].dist(places[b]).total_cmp(&places1[*y].dist(places[b]))).unwrap();
        let twice = welded(&once.faces, &j1, b1, Vec3::new(30.0, 30.0, 0.0)).unwrap();
        assert_eq!(twice.sewn.len(), 1, "both ends meet: the sides are one");
        let j2 = joins(&twice.faces).unwrap();
        assert_eq!((j2.seams.len(), j2.open.len()), (1, 6));
        let (p, q) = seam_ends(&twice.faces, &j2, 0).unwrap();
        assert!(p.dist(Vec3::new(30.0, 0.0, 0.0)) < 1e-9 || q.dist(Vec3::new(30.0, 0.0, 0.0)) < 1e-9);
        assert!(welded(&faces, &j, 999, Vec3::ZERO).is_none());
        assert!(welded(&faces, &j, a, Vec3::new(f64::NAN, 0.0, 0.0)).is_none());
    }
}
