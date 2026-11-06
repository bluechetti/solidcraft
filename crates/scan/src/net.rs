//! A Free Form's net changed as a whole (SurfaceCraft's net tools, after QuickSurface's): a row
//! or column of points put between two (Split), a row at an edge copied beside itself to be
//! dragged out (the Alt-drag and the D grip), the net's points doubled (Increase Resolution), a
//! row taken away, and a row made a crease (the surface runs through it, sharp along it, by a
//! triple knot there) or smooth again.
//!
//! A row is a line of points of one `v` index (all `u`), a column one of one `u` index. Split and
//! Increase Resolution leave the surface exactly as it was (Boehm's knot insertion).

use solidcore_record::Vec3;
use solidcraft_geom::bspline::{Dir, HPoint, NurbsSurface};
use solidcraft_geom::prelude::*;

/// The most points a net has each way.
pub const MOST: usize = 48;

/// A line of a net's points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    /// The points of `v` index `j`, along u.
    Row(usize),
    /// The points of `u` index `i`, along v.
    Column(usize),
}

impl Line {
    /// The direction the line's index counts in: a row is a place along v.
    fn dir(self) -> Dir {
        match self {
            Line::Row(_) => Dir::T,
            Line::Column(_) => Dir::S,
        }
    }

    fn index(self) -> usize {
        match self {
            Line::Row(j) | Line::Column(j) => j,
        }
    }

    fn with(self, index: usize) -> Line {
        match self {
            Line::Row(_) => Line::Row(index),
            Line::Column(_) => Line::Column(index),
        }
    }

    /// The control points on the line (u fastest), on a net `count` across.
    pub fn points(self, count: [usize; 2]) -> Vec<usize> {
        let [nu, nv] = count;
        match self {
            Line::Row(j) => (0..nu).map(|i| j * nu + i).collect(),
            Line::Column(i) => (0..nv).map(|j| j * nu + i).collect(),
        }
    }
}

/// Why a net could not be changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetError {
    TooMany,
    OutsideNet,
    Rational,
    /// A crease needs three points each side of it, and three lines between it and the next.
    CreaseRoom,
    AlreadyCreased,
    NotCreased,
    /// The net has too few points that way to take one away.
    TooFew,
    Failed,
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NetError::TooMany => "the net would have more than 48 points that way",
            NetError::OutsideNet => "that line is not on the net",
            NetError::Rational => "the surface's points have weights: its net cannot be changed so",
            NetError::CreaseRoom => "a crease needs three lines of points each side of it and between it and another crease: Increase Resolution first, or choose a line further in",
            NetError::AlreadyCreased => "that line is a crease already",
            NetError::NotCreased => "that line is not a crease",
            NetError::TooFew => "the net has too few points that way to take a line away",
            NetError::Failed => "the net could not be changed",
        })
    }
}

fn axis(dir: Dir) -> usize {
    match dir {
        Dir::S => 0,
        Dir::T => 1,
    }
}

/// The Greville place of point `i` on `knots` of degree `p`: where it pulls hardest.
pub(crate) fn greville(knots: &[f64], i: usize, p: usize) -> f64 {
    knots.get(i + 1..=i + p).map_or(0.0, |k| k.iter().sum::<f64>() / p as f64)
}

fn non_rational(s: &NurbsSurface) -> Result<(), NetError> {
    let w0 = s.homogeneous().first().map_or(1.0, |h| h.w);
    if s.homogeneous().iter().all(|h| (h.w - w0).abs() <= 1e-12 * w0.abs().max(1.0)) { Ok(()) } else { Err(NetError::Rational) }
}

/// The middle of the widest knot span that way: where a line refines the net most.
fn widest_middle(s: &NurbsSurface, d: usize) -> Option<f64> {
    let (p, n, k) = (s.degree()[d], s.count()[d], &s.knots()[d]);
    (p..n).map(|i| (k[i], k[i + 1])).filter(|(a, b)| b > a).max_by(|x, y| (x.1 - x.0).total_cmp(&(y.1 - y.0))).map(|(a, b)| 0.5 * (a + b))
}

/// The net's points doubled in u, in v or both (QuickSurface's Increase Resolution): a knot half
/// way along every span, the surface exactly as it was.
pub fn refine(s: &NurbsSurface, in_u: bool, in_v: bool) -> Result<NurbsSurface, NetError> {
    let mut made = s.clone();
    for (dir, wanted) in [(Dir::S, in_u), (Dir::T, in_v)] {
        if !wanted {
            continue;
        }
        let d = axis(dir);
        let (n, p, knots) = (made.count()[d], made.degree()[d], made.knots()[d].clone());
        let middles: Vec<f64> = (p..n).filter(|&k| knots[k + 1] > knots[k]).map(|k| 0.5 * (knots[k] + knots[k + 1])).collect();
        if n + middles.len() > MOST {
            return Err(NetError::TooMany);
        }
        for at in middles {
            made = made.insert_knot(dir, at).map_err(|_| NetError::Failed)?;
        }
    }
    Ok(made)
}

/// A line put half way between `line` and its neighbour (the next, or the one before for the last),
/// the surface exactly as it was (QuickSurface's Split). The new line comes back with the surface.
pub fn split(s: &NurbsSurface, line: Line) -> Result<(NurbsSurface, Line), NetError> {
    let d = axis(line.dir());
    let (n, p, knots) = (s.count()[d], s.degree()[d], &s.knots()[d]);
    let index = line.index();
    if index >= n {
        return Err(NetError::OutsideNet);
    }
    if n + 1 > MOST {
        return Err(NetError::TooMany);
    }
    let (a, b) = if index + 1 < n { (index, index + 1) } else { (index - 1, index) };
    let mut at = 0.5 * (greville(knots, a, p) + greville(knots, b, p));
    // Not on a knot already (a repeated knot loses smoothness), and inside: else the widest span.
    if knots.iter().any(|k| (k - at).abs() < 1e-9) || !(at > knots[p] && at < knots[n]) {
        at = widest_middle(s, d).ok_or(NetError::Failed)?;
    }
    let made = s.insert_knot(line.dir(), at).map_err(|_| NetError::Failed)?;
    Ok((made, line.with(a + 1)))
}

/// The line copied beside itself (after it when `after`, else before), the copy lying on the
/// original until it is moved: QuickSurface's Alt-drag on an edge, which drags the copy out. A
/// knot goes in the span nearest the new line, so the surface changes there and nowhere else the
/// net reaches. The copy comes back with the surface.
pub fn duplicate(s: &NurbsSurface, line: Line, after: bool) -> Result<(NurbsSurface, Line), NetError> {
    non_rational(s)?;
    let d = axis(line.dir());
    let [nu, nv] = s.count();
    let (n, p, knots) = (s.count()[d], s.degree()[d], &s.knots()[d]);
    let index = line.index();
    if index >= n {
        return Err(NetError::OutsideNet);
    }
    if n + 1 > MOST || n <= p {
        return Err(NetError::TooMany);
    }
    let new_index = if after { index + 1 } else { index };
    let wanted = new_index.saturating_sub(p.saturating_sub(2)).clamp(p, n - 1);
    let span = (p..n).filter(|&k| knots[k + 1] > knots[k]).min_by_key(|&k| k.abs_diff(wanted)).ok_or(NetError::Failed)?;
    let at = 0.5 * (knots[span] + knots[span + 1]);
    let mut new_knots = knots.clone();
    new_knots.insert(span + 1, at);
    let (mu, mv) = if d == 1 { (nu, nv + 1) } else { (nu + 1, nv) };
    let old = s.homogeneous();
    let mut net: Vec<HPoint> = Vec::with_capacity(mu * mv);
    for j in 0..mv {
        for i in 0..mu {
            let (a, b) = if d == 1 { (i, j) } else { (j, i) };
            let from = if b < new_index {
                b
            } else if b == new_index {
                index
            } else {
                b - 1
            };
            let (oi, oj) = if d == 1 { (a, from) } else { (from, a) };
            net.push(old[oj * nu + oi]);
        }
    }
    let mut all = s.knots().clone();
    all[d] = new_knots;
    let made = NurbsSurface::from_homogeneous(s.degree(), all, net).map_err(|_| NetError::Failed)?;
    Ok((made, line.with(new_index)))
}

/// The line taken away, with the knot nearest it that is not repeated (a crease stays): the
/// undoing of [`duplicate`]. The surface changes where the line reached.
pub fn remove(s: &NurbsSurface, line: Line) -> Result<NurbsSurface, NetError> {
    non_rational(s)?;
    let d = axis(line.dir());
    let [nu, nv] = s.count();
    let (n, p, knots) = (s.count()[d], s.degree()[d], &s.knots()[d]);
    let index = line.index();
    if index >= n {
        return Err(NetError::OutsideNet);
    }
    if n <= p + 1 {
        return Err(NetError::TooFew);
    }
    let g = greville(knots, index, p);
    let single = |k: usize| knots[k] > knots[k - 1] && knots[k + 1] > knots[k];
    let gone = (p + 1..n).filter(|&k| single(k)).min_by(|&a, &b| (knots[a] - g).abs().total_cmp(&(knots[b] - g).abs())).ok_or(NetError::TooFew)?;
    let mut new_knots = knots.clone();
    new_knots.remove(gone);
    let net: Vec<HPoint> = (0..nu * nv).filter(|k| if d == 1 { k / nu != index } else { k % nu != index }).map(|k| s.homogeneous()[k]).collect();
    let mut all = s.knots().clone();
    all[d] = new_knots;
    NurbsSurface::from_homogeneous(s.degree(), all, net).map_err(|_| NetError::Failed)
}

/// The lines that are creases: a triple knot at the line's Greville place (cubic).
pub fn creases(s: &NurbsSurface) -> Vec<Line> {
    let mut out = Vec::new();
    for (d, make) in [(0usize, Line::Column as fn(usize) -> Line), (1, Line::Row as fn(usize) -> Line)] {
        let (n, k) = (s.count()[d], &s.knots()[d]);
        for index in 3..n.saturating_sub(3) {
            if k[index + 1] == k[index + 2] && k[index + 2] == k[index + 3] {
                out.push(make(index));
            }
        }
    }
    out
}

/// The line made a crease (QuickSurface's Crease Edges): the surface runs through it and is sharp
/// along it, smooth either side, by its three knots made one. It needs three lines each side of it
/// and between it and another crease (a cubic net of at least seven points that way).
pub fn crease(s: &NurbsSurface, line: Line) -> Result<NurbsSurface, NetError> {
    let d = axis(line.dir());
    let (n, index) = (s.count()[d], line.index());
    if index >= n {
        return Err(NetError::OutsideNet);
    }
    if s.degree()[d] != 3 || n < 7 || !(3..=n - 4).contains(&index) {
        return Err(NetError::CreaseRoom);
    }
    let existing = creases(s);
    if existing.contains(&line) {
        return Err(NetError::AlreadyCreased);
    }
    if existing.iter().any(|l| axis(l.dir()) == d && l.index().abs_diff(index) < 3) {
        return Err(NetError::CreaseRoom);
    }
    let mut all = s.knots().clone();
    let knots = &mut all[d];
    let middle = knots[index + 2];
    if !(knots[index] < middle && middle < knots[index + 4]) {
        return Err(NetError::CreaseRoom);
    }
    knots[index + 1] = middle;
    knots[index + 3] = middle;
    NurbsSurface::from_homogeneous(s.degree(), all, s.homogeneous().to_vec()).map_err(|_| NetError::Failed)
}

/// The crease at the line taken out: its three knots spread evenly between their neighbours again.
pub fn uncrease(s: &NurbsSurface, line: Line) -> Result<NurbsSurface, NetError> {
    if !creases(s).contains(&line) {
        return Err(NetError::NotCreased);
    }
    let d = axis(line.dir());
    let index = line.index();
    let mut all = s.knots().clone();
    let knots = &mut all[d];
    let (low, high) = (knots[index], knots[index + 4]);
    for k in 1..=3 {
        knots[index + k] = low + (high - low) * k as f64 / 4.0;
    }
    NurbsSurface::from_homogeneous(s.degree(), all, s.homogeneous().to_vec()).map_err(|_| NetError::Failed)
}

/// The line at a side of the net copied out past it by `distance` along the way the surface leaves
/// that side (the row next in to the side), so the surface goes on along its tangent: QuickSurface's
/// Ctrl+Alt-drag. The new side comes back with the surface.
pub fn extend(s: &NurbsSurface, side: Line, distance: f64) -> Result<(NurbsSurface, Line), NetError> {
    let d = axis(side.dir());
    let n = s.count()[d];
    let index = side.index();
    if !(index == 0 || index + 1 == n) {
        return Err(NetError::OutsideNet);
    }
    let after = index + 1 == n;
    let (made, copy) = duplicate(s, side, after)?;
    let count = made.count();
    // The copy and the line next in from it, point for point.
    let inner = if after { copy.with(copy.index() - 2) } else { copy.with(copy.index() + 2) };
    let (moved, from) = (copy.points(count), inner.points(count));
    let mut net = made.homogeneous().to_vec();
    for (&k, &f) in moved.iter().zip(&from) {
        let (Some(here), Some(back)) = (net[k].project(), net[f].project()) else { return Err(NetError::Failed) };
        let away = (here - back).normalized().unwrap_or(Vec3::ZERO);
        net[k] = HPoint::weighted(here + away * distance, net[k].w);
    }
    let made = NurbsSurface::from_homogeneous(made.degree(), made.knots().clone(), net).map_err(|_| NetError::Failed)?;
    Ok((made, copy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::free_form::{net_over, surface};

    fn bumpy() -> NurbsSurface {
        let corners = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0), Vec3::new(40.0, 30.0, 0.0), Vec3::new(0.0, 30.0, 0.0)];
        let mut net = net_over(corners, 5);
        for (k, p) in net.iter_mut().enumerate() {
            p.z = ((k * 7) % 5) as f64 - 2.0;
        }
        surface(&net, 5).unwrap()
    }

    fn same(a: &NurbsSurface, b: &NurbsSurface) -> bool {
        (0..=10).all(|i| {
            (0..=10).all(|j| a.eval(i as f64 / 10.0, j as f64 / 10.0).unwrap().dist(b.eval(i as f64 / 10.0, j as f64 / 10.0).unwrap()) < 1e-9)
        })
    }

    #[test]
    fn refine_and_split_leave_the_surface_as_it_was() {
        let s = bumpy();
        let fine = refine(&s, true, true).unwrap();
        assert_eq!(fine.count(), [7, 7]);
        assert!(same(&s, &fine));
        let (split_s, line) = split(&s, Line::Row(1)).unwrap();
        assert_eq!((split_s.count(), line), ([5, 6], Line::Row(2)));
        assert!(same(&s, &split_s));
        let (last, line) = split(&s, Line::Column(4)).unwrap();
        assert_eq!((last.count(), line), ([6, 5], Line::Column(4)));
        assert_eq!(split(&s, Line::Column(9)).unwrap_err(), NetError::OutsideNet);
    }

    #[test]
    fn a_line_duplicated_then_removed_is_one_line_more_then_back() {
        let s = bumpy();
        let (more, copy) = duplicate(&s, Line::Row(4), true).unwrap();
        assert_eq!((more.count(), copy), ([5, 6], Line::Row(5)));
        let pts = |s: &NurbsSurface| crate::surface_edit::points(s);
        assert_eq!(pts(&more)[5 * 5..], pts(&more)[4 * 5..5 * 5], "the copy lies on the original");
        let back = remove(&more, copy).unwrap();
        assert_eq!(back.count(), [5, 5]);
        // Out past the side: the new side is 5 mm further on, along the way the surface left it.
        let (out, side) = extend(&s, Line::Column(4), 5.0).unwrap();
        assert_eq!((out.count(), side), ([6, 5], Line::Column(5)));
        let p = pts(&out);
        assert!(p[5].x > p[4].x + 4.0, "{:?} {:?}", p[4], p[5]);
    }

    #[test]
    fn a_crease_runs_through_its_line_and_comes_out_again() {
        let s = refine(&bumpy(), true, true).unwrap();
        assert_eq!(crease(&s, Line::Row(1)).unwrap_err(), NetError::CreaseRoom);
        let creased = crease(&s, Line::Row(3)).unwrap();
        assert_eq!(creases(&creased), vec![Line::Row(3)]);
        // Along the crease the surface is the creased row's own curve: moving every other row
        // leaves it where it was.
        let at = creased.knots()[1][5];
        let mut net = crate::surface_edit::points(&creased);
        for (k, p) in net.iter_mut().enumerate() {
            if k / 7 != 3 {
                p.z += 5.0;
            }
        }
        let moved = crate::surface_edit::with_points(&creased, &net).unwrap();
        for k in 0..=20 {
            let u = k as f64 / 20.0;
            assert!(creased.eval(u, at).unwrap().dist(moved.eval(u, at).unwrap()) < 1e-9, "at u = {u}");
        }
        assert_eq!(crease(&creased, Line::Row(3)).unwrap_err(), NetError::AlreadyCreased);
        let smooth = uncrease(&creased, Line::Row(3)).unwrap();
        assert!(creases(&smooth).is_empty());
    }
}
