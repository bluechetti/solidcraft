//! Symmetry for a Free Form (SurfaceCraft's): a network kept a mirror
//! image of itself in a plane. A half modelled on one side is mirrored and joined to its image on
//! the plane ([`mirrored`]); from then on its points come in pairs ([`Pairs`]): every edit is made
//! on both ([`Pairs::moves`]), the network is made an exact mirror image again after each solve
//! ([`Pairs::kept`]), and a point on the plane stays on it. The pairs are read from the points'
//! places alone; a design keeps only the plane (its `free_forms`), and Symmetry turned off can
//! keep one half ([`half_kept`]).

use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_geom::prelude::*;

use crate::network::{self, Joins, NetworkError};
use crate::surface_edit::{points, with_points};

/// A plane to mirror in: a point on it and its unit normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MirrorPlane {
    pub point: Vec3,
    pub normal: Vec3,
}

impl MirrorPlane {
    /// `None` for a point that is not a place, or a normal with no direction.
    pub fn new(point: Vec3, normal: Vec3) -> Option<MirrorPlane> {
        if !(point.is_finite() && normal.is_finite()) {
            return None;
        }
        Some(MirrorPlane { point, normal: normal.normalized()? })
    }

    /// How far `p` is off the plane, signed along the normal.
    pub fn side(&self, p: Vec3) -> f64 {
        (p - self.point).dot(self.normal)
    }

    /// `p`'s mirror image.
    pub fn reflect(&self, p: Vec3) -> Vec3 {
        p - self.normal * (2.0 * self.side(p))
    }

    /// `p` put onto the plane, square to it.
    pub fn onto(&self, p: Vec3) -> Vec3 {
        p - self.normal * self.side(p)
    }
}

/// Why a Free Form could not be mirrored, or paired.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MirrorError {
    /// Points on both sides of the plane that are not mirror images: how far across it reaches.
    Straddles {
        across: f64,
    },
    /// No point within the gap of the plane: how far off the nearest is.
    Apart {
        nearest: f64,
    },
    /// Every point within the gap of the plane.
    OnPlane,
    TooManyFaces,
    Network(NetworkError),
    /// Not a mirror image in the plane: a point with no twin.
    NotMirror,
    Failed,
}

impl std::fmt::Display for MirrorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MirrorError::Straddles { across } => write!(
                f,
                "the Free Form lies on both sides of the plane ({across:.2} mm across it) and is not a mirror image in it: mirror a half, modelled on one side"
            ),
            MirrorError::Apart { nearest } => {
                write!(
                    f,
                    "no edge of the Free Form reaches the plane: the nearest point is {nearest:.2} mm off it (a gap of {:.2} mm would reach it)",
                    1.05 * nearest
                )
            }
            MirrorError::OnPlane => f.write_str("the Free Form lies in the plane: a half stands on it, on one side"),
            MirrorError::TooManyFaces => write!(f, "mirrored, the Free Form would have more than {} faces", network::MOST_FACES),
            MirrorError::Network(e) => write!(f, "{e}"),
            MirrorError::NotMirror => f.write_str("the Free Form is not a mirror image in that plane: mirror a half first"),
            MirrorError::Failed => f.write_str("the Free Form could not be mirrored"),
        }
    }
}

/// The pairs of a network that is a mirror image of itself: for each network point its twin
/// (itself for a point on the plane).
#[derive(Clone, Debug, PartialEq)]
pub struct Pairs {
    pub plane: MirrorPlane,
    pub twin: Vec<usize>,
    pub on_plane: Vec<bool>,
}

impl Pairs {
    /// How many pairs of points there are (a point on the plane is none).
    pub fn pairs(&self) -> usize {
        self.twin.iter().enumerate().filter(|(p, t)| p < t).count()
    }

    /// How many points lie on the plane.
    pub fn on_plane_count(&self) -> usize {
        self.on_plane.iter().filter(|o| **o).count()
    }

    /// Whether point `p` lies on the plane.
    pub fn is_on_plane(&self, p: usize) -> bool {
        self.on_plane.get(p).copied().unwrap_or(false)
    }

    /// `moves` made on both sides: each point's twin goes to the mirror image of where it goes,
    /// and a point on the plane goes onto the plane. A point named after its twin wins.
    pub fn moves(&self, moves: &[(usize, Vec3)]) -> Vec<(usize, Vec3)> {
        let mut out: Vec<(usize, Vec3)> = Vec::with_capacity(2 * moves.len());
        let mut put = |p: usize, to: Vec3| {
            out.retain(|m| m.0 != p);
            out.push((p, to));
        };
        for &(p, to) in moves {
            let Some(&t) = self.twin.get(p) else { continue };
            if self.is_on_plane(p) {
                put(p, self.plane.onto(to));
            } else {
                put(p, to);
                put(t, self.plane.reflect(to));
            }
        }
        out
    }

    /// `free` with every free point's twin free too.
    pub fn both(&self, free: &[bool]) -> Vec<bool> {
        let mut out = free.to_vec();
        for (p, &t) in self.twin.iter().enumerate() {
            if free.get(p).copied().unwrap_or(false)
                && let Some(f) = out.get_mut(t)
            {
                *f = true;
            }
        }
        out
    }

    /// `faces` made an exact mirror image: each pair put at the mean of the two, each point on the
    /// plane put onto it.
    pub fn kept(&self, faces: &[NurbsSurface], joins: &Joins) -> Option<Vec<NurbsSurface>> {
        let mut places = network::network_points(faces, joins);
        if places.len() != self.twin.len() {
            return None;
        }
        for p in 0..places.len() {
            let t = self.twin[p];
            if self.is_on_plane(p) {
                places[p] = self.plane.onto(places[p]);
            } else if p < t && t < places.len() {
                let mean = (places[p] + self.plane.reflect(places[t])) * 0.5;
                places[p] = mean;
                places[t] = self.plane.reflect(mean);
            }
        }
        network::with_network_points(faces, joins, &places)
    }
}

/// A tolerance for pairing the points of `faces`: a quarter of the closest spacing of the
/// network's points, never more than `most`.
pub fn pairing_tolerance(faces: &[NurbsSurface], most: f64) -> f64 {
    let mut closest = f64::INFINITY;
    for face in faces {
        let [nu, nv] = face.count();
        let net = points(face);
        for j in 0..nv {
            for i in 0..nu {
                let here = net[j * nu + i];
                if i + 1 < nu {
                    closest = closest.min(here.dist(net[j * nu + i + 1]));
                }
                if j + 1 < nv {
                    closest = closest.min(here.dist(net[(j + 1) * nu + i]));
                }
            }
        }
    }
    let quarter = if closest.is_finite() { closest / 4.0 } else { most };
    quarter.min(most).max(1e-9)
}

/// The pairs of `faces` in `plane`: every network point's twin among the points (its reflection
/// within `tolerance`), or itself for a point within `tolerance` of the plane. Refused when a
/// point has no twin.
pub fn pairs_of(faces: &[NurbsSurface], joins: &Joins, plane: MirrorPlane, tolerance: f64) -> Result<Pairs, MirrorError> {
    let places = network::network_points(faces, joins);
    let n = places.len();
    if n == 0 || !(tolerance.is_finite() && tolerance >= 0.0) {
        return Err(MirrorError::Failed);
    }
    let mut by_x: Vec<usize> = (0..n).collect();
    by_x.sort_by(|a, b| places[*a].x.total_cmp(&places[*b].x));
    let mut twin = vec![usize::MAX; n];
    let mut on_plane = vec![false; n];
    for p in 0..n {
        if plane.side(places[p]).abs() <= tolerance {
            twin[p] = p;
            on_plane[p] = true;
            continue;
        }
        let r = plane.reflect(places[p]);
        let from = by_x.partition_point(|&q| places[q].x < r.x - tolerance);
        let mut best: Option<(usize, f64)> = None;
        for &q in by_x[from..].iter().take_while(|&&q| places[q].x <= r.x + tolerance) {
            let d = places[q].dist(r);
            if d <= tolerance && best.is_none_or(|b| d < b.1) {
                best = Some((q, d));
            }
        }
        match best {
            Some((q, _)) => twin[p] = q,
            None => return Err(MirrorError::NotMirror),
        }
    }
    if (0..n).any(|p| twin.get(twin[p]).copied() != Some(p)) {
        return Err(MirrorError::NotMirror);
    }
    Ok(Pairs { plane, twin, on_plane })
}

/// `face` reflected in `plane`, its u turned round so that it faces the way the original does
/// (a reflection alone turns a surface inside out).
pub fn mirror_face(face: &NurbsSurface, plane: MirrorPlane) -> Option<NurbsSurface> {
    let [nu, nv] = face.count();
    let net = points(face);
    if net.len() != nu * nv || nu == 0 {
        return None;
    }
    let mut out = Vec::with_capacity(net.len());
    for j in 0..nv {
        for i in 0..nu {
            out.push(plane.reflect(net[j * nu + (nu - 1 - i)]));
        }
    }
    let ku = &face.knots()[0];
    let (lo, hi) = (*ku.first()?, *ku.last()?);
    let turned: Vec<f64> = ku.iter().rev().map(|k| lo + hi - k).collect();
    NurbsSurface::new(face.degree(), [turned, face.knots()[1].clone()], &out, &vec![1.0; out.len()]).ok()
}

/// The face among `faces` that is the mirror image of `face` in `plane` (itself when it is its
/// own): one whose points are the reflections of its points, within `tolerance`.
pub fn twin_face(faces: &[NurbsSurface], plane: MirrorPlane, face: usize, tolerance: f64) -> Option<usize> {
    let f = faces.get(face)?;
    let reflected: Vec<Vec3> = points(f).iter().map(|p| plane.reflect(*p)).collect();
    faces.iter().position(|c| {
        c.count() == f.count() && {
            let net = points(c);
            reflected.iter().all(|q| net.iter().any(|p| p.dist(*q) <= tolerance))
        }
    })
}

/// How [`mirrored`] made the network a mirror image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// It was one already: nothing changed.
    Already,
    /// A half mirrored: `added` faces, `onto` points put onto the plane.
    Mirrored { added: usize, onto: usize },
}

/// `faces` as a mirror image of itself in `plane`: as they are when they are one already; else
/// the half they are (every point on one side of the plane, or within `gap` of it) with its open
/// sides within `gap` put onto the plane and its mirror image added, joined to it there. The
/// pairs of the result.
pub fn mirrored(faces: &[NurbsSurface], plane: MirrorPlane, gap: f64) -> Result<(Vec<NurbsSurface>, Pairs, How), MirrorError> {
    let joins = network::joins(faces).map_err(MirrorError::Network)?;
    let gap = if gap.is_finite() && gap > 0.0 { gap } else { 0.0 };
    if let Ok(pairs) = pairs_of(faces, &joins, plane, pairing_tolerance(faces, gap.max(1e-6))) {
        return Ok((faces.to_vec(), pairs, How::Already));
    }
    let places = network::network_points(faces, &joins);
    let sides: Vec<f64> = places.iter().map(|p| plane.side(*p)).collect();
    let (low, high) = sides.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
    if !(low.is_finite() && high.is_finite()) {
        return Err(MirrorError::Failed);
    }
    // The side the half is on: the one it reaches farther into.
    let sign = if high.abs() >= low.abs() { 1.0 } else { -1.0 };
    let across = sides.iter().map(|s| -s * sign).fold(0.0, f64::max);
    if across > gap {
        return Err(MirrorError::Straddles { across });
    }
    let nearest = sides.iter().map(|s| s.abs()).fold(f64::INFINITY, f64::min);
    if nearest > gap {
        return Err(MirrorError::Apart { nearest });
    }
    if sides.iter().all(|s| s.abs() <= gap) {
        return Err(MirrorError::OnPlane);
    }
    if 2 * faces.len() > network::MOST_FACES {
        return Err(MirrorError::TooManyFaces);
    }
    // The open sides that lie within the gap all along go onto the plane: whole sides only, so
    // that a net whose points crowd its edge (a fitted net's do) never has an inner row put onto
    // the plane beside its edge.
    let mut moved = places.clone();
    let mut onto = 0;
    let mut put: Vec<bool> = vec![false; places.len()];
    for &(f, side) in &joins.open {
        let ids = joins.side(faces, f, side);
        if !ids.is_empty() && ids.iter().all(|p| sides.get(*p).is_some_and(|s| s.abs() <= gap)) {
            for p in ids {
                if let Some(flag) = put.get_mut(p) {
                    *flag = true;
                }
            }
        }
    }
    for (p, flag) in put.iter().enumerate() {
        if *flag {
            moved[p] = plane.onto(places[p]);
            onto += 1;
        }
    }
    if onto == 0 {
        return Err(MirrorError::Apart { nearest });
    }
    let half = network::with_network_points(faces, &joins, &moved).ok_or(MirrorError::Failed)?;
    let mut all = half.clone();
    for f in &half {
        all.push(mirror_face(f, plane).ok_or(MirrorError::Failed)?);
    }
    let joins = network::joins(&all).map_err(MirrorError::Network)?;
    let pairs = pairs_of(&all, &joins, plane, pairing_tolerance(&all, 1e-6))?;
    Ok((all, pairs, How::Mirrored { added: half.len(), onto }))
}

/// `faces` with the face `new` (just added) given its mirror image in `plane` too: its points
/// within `gap` of the plane put onto it first, so the two join there; nothing added when the
/// face is its own mirror image. The network is then made an exact mirror image again (adding a
/// face refits the row of the face it grows from). `None` when it is no mirror image any more.
pub fn with_twin_face(faces: &[NurbsSurface], plane: MirrorPlane, new: usize, gap: f64) -> Option<Vec<NurbsSurface>> {
    let mut faces = faces.to_vec();
    let face = faces.get(new)?;
    let gap = if gap.is_finite() && gap > 0.0 { gap } else { 0.0 };
    let net: Vec<Vec3> = points(face).iter().map(|p| if plane.side(*p).abs() <= gap { plane.onto(*p) } else { *p }).collect();
    let snapped = with_points(face, &net)?;
    faces[new] = snapped;
    if twin_face(&faces, plane, new, pairing_tolerance(&faces, 1e-6)).is_none_or(|t| t != new) {
        let twin = mirror_face(&faces[new], plane)?;
        faces.push(twin);
    }
    made_exact(&faces, plane, gap)
}

/// `faces` (the network `before`, a mirror image in `plane`, with a face added last and the faces
/// it grows from refitted near it) given the added face's mirror image too: each point the
/// adding moved takes its twin to its mirror image first, so the two sides are still each
/// other's, then [`with_twin_face`]. `None` when `before` is no mirror image in `plane`, or the
/// faces are not `before`'s with one added.
pub fn with_twin_face_after(before: &[NurbsSurface], faces: &[NurbsSurface], plane: MirrorPlane, gap: f64) -> Option<Vec<NurbsSurface>> {
    let n = before.len();
    if faces.len() != n + 1 || faces[..n].iter().zip(before).any(|(a, b)| a.count() != b.count()) {
        return None;
    }
    let joins = network::joins(before).ok()?;
    let pairs = pairs_of(before, &joins, plane, pairing_tolerance(before, gap.max(1e-6))).ok()?;
    let was = network::network_points(before, &joins);
    let now = network::network_points(&faces[..n], &joins);
    let mut places = now.clone();
    for p in 0..places.len() {
        let t = pairs.twin.get(p).copied()?;
        let moved = |q: usize| now[q].dist(was[q]) > 1e-12;
        if pairs.is_on_plane(p) {
            places[p] = plane.onto(now[p]);
        } else if moved(p) && !moved(t) {
            places[t] = plane.reflect(now[p]);
        }
    }
    let mut all = network::with_network_points(&faces[..n], &joins, &places)?;
    all.push(faces[n].clone());
    with_twin_face(&all, plane, n, gap)
}

/// `faces` with the half on the far side of `plane` from `keep` taken away: the faces whose points
/// all lie on `keep`'s side (or on the plane) stay, those on the other side go, as Symmetry turned
/// off with Keep One Half does. Refused when a face lies across the plane, when `keep` is on the
/// plane, and when nothing would go or nothing stay.
pub fn half_kept(faces: &[NurbsSurface], plane: MirrorPlane, keep: Vec3) -> Result<Vec<NurbsSurface>, MirrorError> {
    if !keep.is_finite() {
        return Err(MirrorError::Failed);
    }
    let tolerance = network::ends_tolerance(faces).max(1e-6);
    let sign = plane.side(keep);
    if sign.abs() <= tolerance {
        return Err(MirrorError::OnPlane);
    }
    let sign = sign.signum();
    let mut kept = Vec::new();
    for face in faces {
        let sides: Vec<f64> = points(face).iter().map(|p| sign * plane.side(*p)).collect();
        let (low, high) = sides.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), s| (lo.min(*s), hi.max(*s)));
        if low < -tolerance && high > tolerance {
            return Err(MirrorError::Straddles { across: low.abs().min(high) });
        }
        if low >= -tolerance {
            kept.push(face.clone());
        }
    }
    if kept.is_empty() || kept.len() == faces.len() {
        return Err(MirrorError::NotMirror);
    }
    let joins = network::joins(&kept).map_err(MirrorError::Network)?;
    if !network::connected(&kept, &joins) {
        return Err(MirrorError::Network(NetworkError::WouldSplit));
    }
    Ok(kept)
}

/// `faces`, nearly a mirror image in `plane` (each point within `gap`, and within a quarter of
/// the net's spacing, of its twin), made an exact one. `None` when they are not.
pub fn made_exact(faces: &[NurbsSurface], plane: MirrorPlane, gap: f64) -> Option<Vec<NurbsSurface>> {
    let joins = network::joins(faces).ok()?;
    let pairs = pairs_of(faces, &joins, plane, pairing_tolerance(faces, gap.max(1e-6))).ok()?;
    pairs.kept(faces, &joins)
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

    /// A dome, a mirror image of itself in x = 60.
    fn dome(x: f64, y: f64) -> f64 {
        10.0 - 0.002 * ((x - 60.0).powi(2) + 2.0 * (y - 30.0).powi(2))
    }

    fn snapper(on: &Projector) -> impl Fn(&[Vec3], Option<Vec3>) -> Vec<Option<Vec3>> + '_ {
        |ps: &[Vec3], along: Option<Vec3>| {
            ps.iter().map(|p| along.and_then(|d| on.along(*p, d)).map(|(q, _)| q).or_else(|| on.nearest(*p).map(|(q, _)| q))).collect()
        }
    }

    fn fitted(on: &Projector, x0: f64, x1: f64) -> NurbsSurface {
        let corners = [Vec3::new(x0, 10.0, 0.0), Vec3::new(x1, 10.0, 0.0), Vec3::new(x1, 50.0, 0.0), Vec3::new(x0, 50.0, 0.0)];
        crate::free_form::fitted(corners, &crate::free_form::Fitting { count: 5, ..Default::default() }, snapper(on)).unwrap().surface
    }

    fn plane() -> MirrorPlane {
        MirrorPlane::new(Vec3::new(60.0, 0.0, 0.0), Vec3::X).unwrap()
    }

    /// A half fitted on one side of the dome's mirror plane, its edge a little short of it, is
    /// mirrored: two faces joined on the plane, every point paired or on the plane, the image
    /// facing the way the half does, the two meeting smoothly. Mirrored again it is one already.
    #[test]
    fn a_half_mirrored_is_one_network_joined_on_the_plane() {
        let (positions, triangles) = field(60, dome);
        let scan = Scan::new(&positions, &triangles);
        let on = Projector::new(&scan);
        let half = fitted(&on, 20.0, 59.6);
        let (faces, pairs, how) = mirrored(std::slice::from_ref(&half), plane(), 1.0).unwrap();
        assert_eq!((faces.len(), how), (2, How::Mirrored { added: 1, onto: 5 }));
        let joins = network::joins(&faces).unwrap();
        assert_eq!((joins.seams.len(), joins.open.len()), (1, 6), "joined on the plane");
        assert_eq!((pairs.pairs(), pairs.on_plane_count()), (20, 5));
        let places = network::network_points(&faces, &joins);
        for (p, &t) in pairs.twin.iter().enumerate() {
            let want = if pairs.is_on_plane(p) { plane().onto(places[p]) } else { plane().reflect(places[t]) };
            assert!(places[p].dist(want) < 1e-9, "{p} ↔ {t}: {:?} against {want:?}", places[p]);
        }
        assert!(network::worst_kink(&faces, &joins) < 0.5, "{}", network::worst_kink(&faces, &joins));
        assert!(!joins.smooth.is_empty(), "held tangent across the plane");
        let under = surface_edit::covered(&scan, &faces[1], 2.0, 5_000);
        assert!(surface_edit::distance(&faces[1], &under).0 < 0.1, "the image lies on the dome too");
        let (again, _, how) = mirrored(&faces, plane(), 1.0).unwrap();
        assert_eq!((again.len(), how), (2, How::Already));
        assert_eq!(twin_face(&faces, plane(), 0, 1e-6), Some(1));
        assert_eq!(twin_face(&faces, plane(), 1, 1e-6), Some(0));
    }

    /// Edits are made on both sides: a dot moved takes its twin to the mirror image; a point on
    /// the plane stays on it however it is sent; the network is made exact again after a solve.
    #[test]
    fn edits_made_in_pairs_keep_the_network_a_mirror_image() {
        let (positions, triangles) = field(60, dome);
        let on = Projector::new(&Scan::new(&positions, &triangles));
        let (faces, pairs, _) = mirrored(&[fitted(&on, 20.0, 59.6)], plane(), 1.0).unwrap();
        let joins = network::joins(&faces).unwrap();
        let dots = network::dots(&faces, &joins);
        let inner = (0..dots.len())
            .find(|&p| !pairs.is_on_plane(p) && dots[p].at.x < 45.0 && dots[p].at.x > 35.0 && (dots[p].at.y - 30.0).abs() < 3.0)
            .unwrap();
        let moves = pairs.moves(&[(inner, dots[inner].at + Vec3::new(0.0, 0.0, 3.0))]);
        assert_eq!(moves.len(), 2);
        let moved = network::dots_moved(&faces, &joins, &moves, network::DOT_RING, true, &[]).unwrap();
        let kept = pairs.kept(&moved, &joins).unwrap();
        let after = network::dots(&kept, &joins);
        let twin = pairs.twin[inner];
        assert!(after[inner].at.z - dots[inner].at.z > 2.9, "rose: {:?}", after[inner].at);
        assert!(after[twin].at.dist(plane().reflect(after[inner].at)) < 1e-6, "its twin its mirror image");
        let on_plane = (0..dots.len()).find(|&p| pairs.is_on_plane(p)).unwrap();
        let sent = pairs.moves(&[(on_plane, dots[on_plane].at + Vec3::new(4.0, 0.0, 2.0))]);
        assert_eq!(sent.len(), 1);
        assert!(plane().side(sent[0].1).abs() < 1e-9 && (sent[0].1.z - dots[on_plane].at.z - 2.0).abs() < 1e-9, "onto the plane, up by 2");
        let free = pairs.both(&(0..joins.len()).map(|p| p == inner).collect::<Vec<_>>());
        assert!(free[inner] && free[twin] && free.iter().filter(|f| **f).count() == 2);
        assert!(pairs_of(&kept, &joins, plane(), 1e-6).is_ok(), "exact again");
    }

    /// What is not a half is refused, each with its reason; a nearly symmetric network is paired;
    /// hostile planes and gaps never panic.
    #[test]
    fn what_is_not_a_half_is_refused_and_hostile_input_never_panics() {
        let (positions, triangles) = field(60, dome);
        let on = Projector::new(&Scan::new(&positions, &triangles));
        // Fitted across the plane, not a mirror image in it (one side reaches 20 mm farther).
        let straddling = fitted(&on, 30.0, 80.0);
        assert!(matches!(mirrored(std::slice::from_ref(&straddling), plane(), 1.0), Err(MirrorError::Straddles { across }) if across > 15.0));
        let apart = fitted(&on, 10.0, 40.0);
        let err = mirrored(&[apart], plane(), 1.0).unwrap_err();
        assert!(matches!(err, MirrorError::Apart { nearest } if nearest > 15.0), "{err}");
        assert!(err.to_string().contains("mm off"));
        // Nearly a mirror image of itself in x = 60 (fitted over the dome's middle): its own twin.
        let whole = fitted(&on, 40.0, 80.0);
        let joins = network::joins(std::slice::from_ref(&whole)).unwrap();
        let pairs = pairs_of(std::slice::from_ref(&whole), &joins, plane(), 0.5).unwrap();
        assert_eq!((pairs.on_plane_count(), pairs.pairs()), (5, 10));
        assert!(MirrorPlane::new(Vec3::new(f64::NAN, 0.0, 0.0), Vec3::X).is_none());
        assert!(MirrorPlane::new(Vec3::ZERO, Vec3::ZERO).is_none());
        assert!(MirrorPlane::new(Vec3::ZERO, Vec3::new(f64::NAN, 0.0, 0.0)).is_none());
        for gap in [f64::NAN, -1.0, f64::INFINITY] {
            let _ = mirrored(std::slice::from_ref(&straddling), plane(), gap);
        }
        assert!(pairs_of(&[], &Joins::default(), plane(), 1.0).is_err());
        assert!(twin_face(std::slice::from_ref(&whole), plane(), 7, 1e-6).is_none());
        assert!(pairs.kept(&[], &joins).is_none_or(|f| f.is_empty()));
        assert!(pairs.moves(&[(99, Vec3::ZERO)]).is_empty());
    }

    /// A face added on an open edge of a mirrored network brings its mirror image; one added on
    /// an edge that is its own mirror image is its own.
    #[test]
    fn a_face_added_brings_its_mirror_image() {
        let (positions, triangles) = field(60, dome);
        let on = Projector::new(&Scan::new(&positions, &triangles));
        let (faces, _, _) = mirrored(&[fitted(&on, 20.0, 59.6)], plane(), 1.0).unwrap();
        let joins = network::joins(&faces).unwrap();
        // The half's far side, at x = 20: a face out to x = 5.
        let (f, side) = *joins.open.iter().find(|&&(f, s)| s.at(&faces[f], 0.5).is_some_and(|p| (p.x - 20.0).abs() < 1.0)).unwrap();
        let grown = network::with_face(&faces, &joins, f, side, [Vec3::new(5.0, 10.0, 0.0), Vec3::new(5.0, 50.0, 0.0)], 5, snapper(&on)).unwrap();
        let both = with_twin_face(&grown, plane(), grown.len() - 1, 1.0).unwrap();
        assert_eq!(both.len(), 4);
        let joins = network::joins(&both).unwrap();
        assert!(pairs_of(&both, &joins, plane(), 1e-6).is_ok(), "a mirror image still");
        assert!(network::worst_kink(&both, &joins) < 0.5);
    }

    /// Keep One Half: the faces on the side of the point given stay, their mirror images go; a
    /// point on the plane, a face across it, and a half with nothing to take away are refused.
    #[test]
    fn keep_one_half_takes_the_other_side_away() {
        let (positions, triangles) = field(60, dome);
        let on = Projector::new(&Scan::new(&positions, &triangles));
        let half = fitted(&on, 20.0, 59.6);
        let (faces, _, _) = mirrored(std::slice::from_ref(&half), plane(), 1.0).unwrap();
        let left = half_kept(&faces, plane(), Vec3::new(30.0, 30.0, 0.0)).unwrap();
        assert_eq!(left.len(), 1);
        assert!(points(&left[0]).iter().all(|p| p.x <= 60.0 + 1e-9), "the -x half kept");
        let right = half_kept(&faces, plane(), Vec3::new(90.0, 30.0, 0.0)).unwrap();
        assert!(points(&right[0]).iter().all(|p| p.x >= 60.0 - 1e-9), "the +x half kept");
        assert_eq!(half_kept(&faces, plane(), Vec3::new(60.0, 0.0, 0.0)), Err(MirrorError::OnPlane));
        assert!(matches!(half_kept(&[fitted(&on, 30.0, 80.0)], plane(), Vec3::new(30.0, 0.0, 0.0)), Err(MirrorError::Straddles { .. })));
        assert_eq!(half_kept(&left, plane(), Vec3::new(30.0, 30.0, 0.0)), Err(MirrorError::NotMirror), "nothing to take away");
        assert_eq!(half_kept(&faces, plane(), Vec3::new(f64::NAN, 0.0, 0.0)), Err(MirrorError::Failed));
    }
}
