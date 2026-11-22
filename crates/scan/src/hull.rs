//! Convex hulls: of points on a plane (Andrew's monotone chain) and of points in space (built by
//! adding the points one at a time).

use std::collections::{HashMap, HashSet};

use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;

/// The indices of the corners of the convex hull of `points`, counter-clockwise, each corner the
/// point it was. Fewer than three for points that do not spread over the plane.
pub(crate) fn hull2(points: &[Vec2]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&a, &b| points[a].x.total_cmp(&points[b].x).then(points[a].y.total_cmp(&points[b].y)));
    let turn = |o: usize, a: usize, b: usize| (points[a] - points[o]).cross(points[b] - points[o]);
    let mut lower: Vec<usize> = Vec::new();
    for &i in &order {
        while let [.., p, q] = lower[..] {
            if turn(p, q, i) > 0.0 {
                break;
            }
            lower.pop();
        }
        lower.push(i);
    }
    let mut upper: Vec<usize> = Vec::new();
    for &i in order.iter().rev() {
        while let [.., p, q] = upper[..] {
            if turn(p, q, i) > 0.0 {
                break;
            }
            upper.pop();
        }
        upper.push(i);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

/// A face of a hull in space: a triangle of point indices, wound counter-clockwise seen from
/// outside, its outward unit normal and `normal · p` for its points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Face {
    pub a: usize,
    pub b: usize,
    pub c: usize,
    pub normal: Vec3,
    pub offset: f64,
}

/// The faces of the convex hull of `points`; `None` for fewer than four points or points that do
/// not span a solid (to `tolerance`).
///
/// Points on a turned scan tie (many lie within `tolerance` of a face between two faces they see),
/// so a point is added only where the hull stays closed: the faces it sees must have a rim that is
/// one loop through distinct corners. Where they do not, the faces it sees or is level with are
/// tried, and failing that the point is left out (it lies within a few tolerances of the hull).
/// The hull therefore stays closed, with at most `2n − 4` faces.
pub(crate) fn hull3(points: &[Vec3], tolerance: f64) -> Option<Vec<Face>> {
    if points.len() < 4 {
        return None;
    }
    let (mut i0, mut i1) = (0, 0);
    for (i, p) in points.iter().enumerate() {
        if p.x < points[i0].x {
            i0 = i;
        }
        if p.x > points[i1].x {
            i1 = i;
        }
    }
    let direction = (points[i1] - points[i0]).normalized()?;
    if !((points[i1] - points[i0]).len() > tolerance) {
        return None;
    }
    let off_line = |k: usize| (points[k] - points[i0]).cross(direction).len();
    let i2 = (0..points.len()).max_by(|&x, &y| off_line(x).total_cmp(&off_line(y)))?;
    if !(off_line(i2) > tolerance) {
        return None;
    }
    let plane = (points[i1] - points[i0]).cross(points[i2] - points[i0]).normalized()?;
    let off_plane = |k: usize| (points[k] - points[i0]).dot(plane).abs();
    let i3 = (0..points.len()).max_by(|&x, &y| off_plane(x).total_cmp(&off_plane(y)))?;
    if !(off_plane(i3) > tolerance) {
        return None;
    }
    let inside = (points[i0] + points[i1] + points[i2] + points[i3]) / 4.0;

    let face = |a: usize, b: usize, c: usize| -> Option<Face> {
        let mut normal = (points[b] - points[a]).cross(points[c] - points[a]).normalized()?;
        let (b, c) = if normal.dot(points[a] - inside) < 0.0 {
            normal = -normal;
            (c, b)
        } else {
            (b, c)
        };
        Some(Face { a, b, c, normal, offset: normal.dot(points[a]) })
    };
    let mut faces: Vec<Face> = [face(i0, i1, i2), face(i0, i1, i3), face(i0, i2, i3), face(i1, i2, i3)].into_iter().flatten().collect();
    if faces.len() != 4 {
        return None;
    }
    let seed: HashSet<usize> = [i0, i1, i2, i3].into_iter().collect();
    // The hull with the faces `p` is more than `level` above replaced by faces from their rim to
    // `p`; `None` unless it stays closed.
    let cap = |faces: &[Face], p: usize, level: f64| -> Option<Vec<Face>> {
        let at = points[p];
        let (removed, mut kept): (Vec<Face>, Vec<Face>) = faces.iter().partition(|f| f.normal.dot(at) - f.offset > level);
        let edges: HashSet<(usize, usize)> = removed.iter().flat_map(|f| [(f.a, f.b), (f.b, f.c), (f.c, f.a)]).collect();
        let rim: Vec<(usize, usize)> = edges.iter().copied().filter(|&(a, b)| !edges.contains(&(b, a))).collect();
        if !closes(&rim) {
            return None;
        }
        let added: Vec<Face> = rim.iter().filter_map(|&(a, b)| face(a, b, p).filter(|f| f.b == b && f.c == p)).collect();
        if added.len() != rim.len() {
            return None;
        }
        kept.extend(added);
        Some(kept)
    };
    // Within the ball about `inside` that the hull so far holds, a point is inside without
    // looking at any face.
    let reach = |faces: &[Face]| faces.iter().map(|f| f.offset - f.normal.dot(inside)).fold(f64::INFINITY, f64::min);
    let mut ball = reach(&faces);
    for p in (0..points.len()).filter(|p| !seed.contains(p)) {
        if faces.len() > 2 * points.len() {
            return None;
        }
        let at = points[p];
        if at.dist(inside) < ball || !faces.iter().any(|f| f.normal.dot(at) - f.offset > tolerance) {
            continue;
        }
        let Some(next) = cap(&faces, p, tolerance).or_else(|| cap(&faces, p, -tolerance)) else { continue };
        faces = next;
        ball = reach(&faces);
    }
    Some(faces)
}

/// Whether the rim edges `(a, b)` are one loop through distinct corners.
fn closes(rim: &[(usize, usize)]) -> bool {
    let Some(&(start, _)) = rim.first() else { return false };
    let mut next: HashMap<usize, usize> = HashMap::with_capacity(rim.len());
    for &(a, b) in rim {
        if next.insert(a, b).is_some() {
            return false;
        }
    }
    let (mut at, mut steps) = (start, 0);
    while steps <= rim.len() {
        let Some(&to) = next.get(&at) else { return false };
        at = to;
        steps += 1;
        if at == start {
            break;
        }
    }
    at == start && steps == rim.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cube_has_twelve_outward_faces_and_a_flat_set_none() {
        let mut points: Vec<Vec3> = (0..8).map(|i| Vec3::new(f64::from(i & 1), f64::from((i >> 1) & 1), f64::from((i >> 2) & 1))).collect();
        points.push(Vec3::new(0.5, 0.5, 0.5));
        let faces = hull3(&points, 1e-12).unwrap();
        assert_eq!(faces.len(), 12);
        for f in &faces {
            assert!(points.iter().all(|p| f.normal.dot(*p) - f.offset <= 1e-12));
            assert!(f.a != 8 && f.b != 8 && f.c != 8, "the middle point is inside");
        }
        assert!(hull3(&points[..4], 1e-12).is_none(), "four corners of one square span no solid");
    }

    /// Points on rings, as a turned scan has, tie everywhere: the hull still closes, each edge
    /// shared by exactly two faces, with every point within a few tolerances of it.
    #[test]
    fn a_turned_set_of_points_gives_a_closed_hull() {
        let mut points = Vec::new();
        for ring in 0..12 {
            let z = f64::from(ring) * 5.0;
            for k in 0..48 {
                let a = std::f64::consts::TAU * f64::from(k) / 48.0;
                points.push(Vec3::new(20.0 * a.cos(), 20.0 * a.sin(), z));
            }
        }
        let tolerance = 1e-6;
        let faces = hull3(&points, tolerance).unwrap();
        let mut edges: HashMap<(usize, usize), usize> = HashMap::new();
        for f in &faces {
            for e in [(f.a, f.b), (f.b, f.c), (f.c, f.a)] {
                *edges.entry(e).or_default() += 1;
            }
        }
        assert!(edges.iter().all(|(&(a, b), &n)| n == 1 && edges.get(&(b, a)) == Some(&1)));
        let worst = points.iter().map(|p| faces.iter().map(|f| f.normal.dot(*p) - f.offset).fold(f64::MIN, f64::max)).fold(f64::MIN, f64::max);
        assert!(worst <= 10.0 * tolerance, "a point {worst} outside");
    }

    #[test]
    fn a_square_with_a_middle_point_has_four_corners() {
        let points = [Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(0.5, 0.5), Vec2::new(1.0, 1.0), Vec2::new(0.0, 1.0)];
        let mut hull = hull2(&points);
        hull.sort_unstable();
        assert_eq!(hull, [0, 1, 3, 4]);
    }
}
