//! Metrology on a scan: the datum frame of three planes, and wall thickness by rays.
//!
//! A part is measured from its datums: the first plane (the primary one, the face it rests on)
//! gives the frame its Z, the second (turned to be square to the first) its X, and the third only
//! where the frame's origin is, at the corner the three planes meet in. The part lies on the
//! inner side of each plane, so it stands in the frame's positive corner when the planes are the
//! faces of a box it sits in. Squareness is reported, not forced: three real faces are never
//! exactly square to each other.
//!
//! Wall thickness is read as a person reads it with a probe: from a point on one face of the wall
//! a ray goes straight into the material, square to the face, and where it leaves is the other
//! face. A wall is read only where the ray finds a face looking away from it (the way out of the
//! material), so a gap is never taken for a wall.

use solidcore_record::Vec3;
use solidcraft_geom::Mesh;
use solidcraft_geom::MeshIndex;
use solidcraft_geom::prelude::*;

use crate::Scan;
use crate::place::Rigid;

/// Why a datum frame or a thickness could not be found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MetrologyError {
    /// A plane's point or normal is not a finite number, or its normal has no length.
    BadPlane(usize),
    /// The first two planes are within about six degrees of parallel (0-based indices).
    NearlyParallel(usize, usize),
    /// The three planes do not meet at one corner: two are nearly parallel or all share a line.
    NoCorner,
    /// The scan has no triangles with an area.
    NoSurface,
    /// Nothing to measure: no probes and no samples.
    NothingAsked,
}

impl std::fmt::Display for MetrologyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetrologyError::BadPlane(i) => write!(f, "plane {} has no usable point and direction", i + 1),
            MetrologyError::NearlyParallel(a, b) => {
                write!(f, "planes {} and {} are nearly parallel: pick faces that stand at an angle to each other", a + 1, b + 1)
            }
            MetrologyError::NoCorner => f.write_str("the three planes do not meet at one corner: pick three faces of a corner"),
            MetrologyError::NoSurface => f.write_str("the scan has no triangles to measure"),
            MetrologyError::NothingAsked => f.write_str("give points to probe, or a number of samples"),
        }
    }
}

impl std::error::Error for MetrologyError {}

/// A datum plane: a point on it and its direction (either way: the part decides the side).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DatumPlane {
    pub point: Vec3,
    pub normal: Vec3,
}

/// The frame three datum planes make, and the move that brings the scan into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Datum {
    /// The move that lays the scan so the frame is the world's: its origin at the world's, its
    /// axes along the world's.
    pub moved: Rigid,
    /// The corner the three planes meet in, as the scan lies before the move.
    pub origin: Vec3,
    /// The frame's X, Y and Z, as the scan lies before the move.
    pub axes: [Vec3; 3],
    /// The planes' directions toward the part, unit, in the order given.
    pub normals: [Vec3; 3],
    /// The angles between the planes (1st and 2nd, 1st and 3rd, 2nd and 3rd), in degrees.
    pub angles: [f64; 3],
    /// How far the worst of those is from a right angle, in degrees.
    pub off_square: f64,
    /// Whether the part lies on the positive side of the third plane. It does not when the
    /// three planes, taken in this order with the part inside, make a left-handed corner: the
    /// part then lies at negative Y.
    pub third_inside: bool,
}

/// How near parallel two planes may be: the sine of the angle between them.
const LEAST_SINE: f64 = 0.1;

/// The frame of three planes, with `inside` a point on the part's side of all three (the middle
/// of the scan does for a part the planes are faces of).
pub fn datum(planes: &[DatumPlane; 3], inside: Vec3) -> Result<Datum, MetrologyError> {
    if !inside.is_finite() {
        return Err(MetrologyError::NoSurface);
    }
    let mut n = [Vec3::ZERO; 3];
    for (i, (slot, plane)) in n.iter_mut().zip(planes).enumerate() {
        if !plane.point.is_finite() {
            return Err(MetrologyError::BadPlane(i));
        }
        let unit = plane.normal.normalized().filter(|u| u.is_finite()).ok_or(MetrologyError::BadPlane(i))?;
        *slot = if (inside - plane.point).dot(unit) < 0.0 { -unit } else { unit };
    }
    let [n1, n2, n3] = n;
    if n1.cross(n2).len() < LEAST_SINE {
        return Err(MetrologyError::NearlyParallel(0, 1));
    }
    let det = n1.dot(n2.cross(n3));
    if !(det.abs() >= LEAST_SINE * LEAST_SINE) {
        return Err(MetrologyError::NoCorner);
    }
    // The corner: n_i · o = n_i · p_i for the three planes, by Cramer's rule.
    let d = |i: usize| planes.get(i).map_or(0.0, |p| n.get(i).copied().unwrap_or(Vec3::ZERO).dot(p.point));
    let origin = (n2.cross(n3) * d(0) + n3.cross(n1) * d(1) + n1.cross(n2) * d(2)) / det;
    let z = n1;
    let x = (n2 - z * n2.dot(z)).normalized().ok_or(MetrologyError::NearlyParallel(0, 1))?;
    let y = z.cross(x);
    let rotation = [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]];
    let turn = Rigid { rotation, translation: Vec3::ZERO };
    let moved = Rigid { rotation, translation: -turn.turned(origin) };
    let angle = |a: Vec3, b: Vec3| a.dot(b).clamp(-1.0, 1.0).acos().to_degrees();
    let angles = [angle(n1, n2), angle(n1, n3), angle(n2, n3)];
    let off_square = angles.iter().map(|a| (a - 90.0).abs()).fold(0.0, f64::max);
    Ok(Datum { moved, origin, axes: [x, y, z], normals: n, angles, off_square, third_inside: n3.dot(y) > 0.0 })
}

/// What to measure, and how.
#[derive(Clone, Debug, Default)]
pub struct WallSettings {
    /// Points on the scan to read the wall at, one reading each.
    pub probes: Vec<Vec3>,
    /// How many places to read over the whole surface (spread evenly by area, the same places
    /// every time), 0 for none.
    pub samples: usize,
    /// The scan is wound inside out: the rays go the other way.
    pub flip: bool,
    /// The longest wall to read: a ray that goes further is no wall (a solid block, the far side
    /// of a part).
    pub up_to: Option<f64>,
    /// The walls thinner than this are listed.
    pub below: Option<f64>,
}

/// The most thin places a [`Wall`] lists.
pub const MOST_THIN: usize = 20;

/// A wall read at one place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The point on the face the ray started from.
    pub at: Vec3,
    /// Where it left the material.
    pub across: Vec3,
    /// The face's direction there, out of the material.
    pub normal: Vec3,
    pub thickness: f64,
}

/// Why a place gave no reading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// The ray found no face (an open scan, a wall of one face) or none within `up_to`.
    NoWall,
    /// The face it found looks back at the ray: there is no material between (the scan may be
    /// wound inside out, or the face is one side of a gap).
    LookingBack,
    /// The place has no face of the scan under it with an area.
    NoFace,
}

/// The walls of a scan as read.
#[derive(Clone, Debug, PartialEq)]
pub struct Wall {
    /// The places read over the surface that gave a reading, and those that did not.
    pub measured: usize,
    pub no_wall: usize,
    pub looking_back: usize,
    /// The thinnest and the thickest reading of the surface's samples.
    pub thinnest: Option<Reading>,
    pub thickest: Option<Reading>,
    /// Over the surface's samples: the mean, and the 5th, 50th and 95th percentile.
    pub mean: f64,
    pub p05: f64,
    pub median: f64,
    pub p95: f64,
    /// The samples under `below`, thinnest first, none within three times `below` of a thinner
    /// one, at most [`MOST_THIN`].
    pub thin: Vec<Reading>,
    /// The share of the samples under `below`.
    pub share_below: Option<f64>,
    /// What each probe read, in the order given.
    pub probes: Vec<Result<Reading, Miss>>,
}

/// The least cosine between a ray and the face it leaves through for that face to count as the wall's other side (60 degrees).
const MOST_SLANT_COS: f64 = 0.5;

struct Rays<'a> {
    scan: &'a Scan<'a>,
    mesh: &'a Mesh,
    index: &'a MeshIndex,
    flip: bool,
    up_to: Option<f64>,
    start: f64,
}

impl Rays<'_> {
    /// The face of triangle `t`, out of the material by its winding: a point and a unit normal.
    fn face(&self, t: usize) -> Option<(Vec3, Vec3)> {
        let [a, b, c] = self.scan.corners(t)?;
        let normal = (b - a).cross(c - a).normalized()?;
        Some((a, normal))
    }

    /// The wall under `at` on triangle `t`.
    fn read(&self, at: Vec3, t: usize) -> Result<Reading, Miss> {
        let (_, out) = self.face(t).ok_or(Miss::NoFace)?;
        let dir = if self.flip { out } else { -out };
        let origin = at + dir * self.start;
        let (along, hit) = self.index.raycast(self.mesh, origin, dir).ok_or(Miss::NoWall)?;
        let thickness = along + self.start;
        if self.up_to.is_some_and(|most| thickness > most) {
            return Err(Miss::NoWall);
        }
        let (_, far) = self.face(hit).ok_or(Miss::NoWall)?;
        // The ray leaves the material through a face whose outside is along the ray: with the
        // scan wound as it should be, `far` points the way the ray goes.
        let leaves = if self.flip { -far.dot(dir) } else { far.dot(dir) };
        if leaves <= 0.0 {
            return Err(Miss::LookingBack);
        }
        // A face the ray only grazes (over 60 degrees off square) is a fold or an edge of the
        // same skin, not the wall's other side: on a real scan these gave readings of a few hundredths of a millimetre.
        if leaves < MOST_SLANT_COS {
            return Err(Miss::NoWall);
        }
        let normal = if self.flip { -out } else { out };
        Ok(Reading { at, across: origin + dir * along, normal, thickness })
    }
}

/// The places spread over the surface by area: `count` points on triangles, each with its
/// triangle, the same ones every time (a low-discrepancy walk, no random numbers).
fn spread(scan: &Scan, count: usize) -> Vec<(Vec3, usize)> {
    let mut running = Vec::with_capacity(scan.triangle_count());
    let mut total = 0.0;
    for t in 0..scan.triangle_count() {
        let area = scan.corners(t).map_or(0.0, |[a, b, c]| (b - a).cross(c - a).len() / 2.0);
        total += if area.is_finite() && area > 1e-18 { area } else { 0.0 };
        running.push(total);
    }
    if !(total > 0.0) {
        return Vec::new();
    }
    // The golden-ratio sequences in two dimensions (the plastic constant's), folded into a triangle.
    const G1: f64 = 0.754_877_666_246_692_7;
    const G2: f64 = 0.569_840_290_998_053_2;
    (0..count)
        .filter_map(|k| {
            let want = total * (k as f64 + 0.5) / count as f64;
            let t = running.partition_point(|&r| r < want).min(running.len().saturating_sub(1));
            let [a, b, c] = scan.corners(t)?;
            let (mut u, mut v) = ((0.5 + k as f64 * G1).fract(), (0.5 + k as f64 * G2).fract());
            if u + v > 1.0 {
                (u, v) = (1.0 - u, 1.0 - v);
            }
            Some((a + (b - a) * u + (c - a) * v, t))
        })
        .collect()
}

/// The value at share `q` (0 to 1) of the sorted `values`.
fn at_share(values: &[f64], q: f64) -> f64 {
    let last = values.len().saturating_sub(1);
    values.get(((last as f64) * q).round() as usize).copied().unwrap_or(0.0)
}

/// Read the walls of `scan`: at the points probed, and at samples spread over its surface.
pub fn wall(scan: &Scan, settings: &WallSettings) -> Result<Wall, MetrologyError> {
    if settings.probes.is_empty() && settings.samples == 0 {
        return Err(MetrologyError::NothingAsked);
    }
    let mesh = Mesh { positions: scan.positions.to_vec(), triangles: scan.triangles.to_vec(), ..Mesh::default() };
    let index = MeshIndex::triangles_only(&mesh);
    let rays = Rays { scan, mesh: &mesh, index: &index, flip: settings.flip, up_to: settings.up_to, start: (scan.extent() * 1e-6).max(1e-9) };

    let probes = settings
        .probes
        .iter()
        .map(|&p| {
            let t = crate::base::nearest_triangle(scan, p).ok_or(Miss::NoFace)?;
            let [a, b, c] = scan.corners(t).ok_or(Miss::NoFace)?;
            rays.read(solidcraft_geom::compare::closest_on_triangle(p, a, b, c), t)
        })
        .collect();

    let places = spread(scan, settings.samples);
    if settings.samples > 0 && places.is_empty() {
        return Err(MetrologyError::NoSurface);
    }
    let read = crate::par::map(places.len(), |i| places.get(i).map_or(Err(Miss::NoFace), |&(at, t)| rays.read(at, t)));
    let found: Vec<Reading> = read.iter().filter_map(|r| r.as_ref().ok().copied()).collect();
    let no_wall = read.iter().filter(|r| matches!(r, Err(Miss::NoWall))).count();
    let looking_back = read.iter().filter(|r| matches!(r, Err(Miss::LookingBack))).count();

    let mut sorted: Vec<f64> = found.iter().map(|r| r.thickness).collect();
    sorted.sort_by(f64::total_cmp);
    let mean = if sorted.is_empty() { 0.0 } else { sorted.iter().sum::<f64>() / sorted.len() as f64 };
    let (mut thin, mut under) = (Vec::new(), 0usize);
    if let Some(below) = settings.below.filter(|b| *b > 0.0) {
        let mut thinner: Vec<&Reading> = found.iter().filter(|r| r.thickness < below).collect();
        under = thinner.len();
        thinner.sort_by(|a, b| a.thickness.total_cmp(&b.thickness));
        for r in thinner {
            if thin.len() >= MOST_THIN {
                break;
            }
            if thin.iter().all(|k: &Reading| k.at.dist(r.at) >= 3.0 * below) {
                thin.push(*r);
            }
        }
    }
    let share_below = settings.below.filter(|b| *b > 0.0).map(|_| if found.is_empty() { 0.0 } else { under as f64 / found.len() as f64 });
    let by_thickness = |a: &&Reading, b: &&Reading| a.thickness.total_cmp(&b.thickness);
    Ok(Wall {
        measured: found.len(),
        no_wall,
        looking_back,
        thinnest: found.iter().min_by(by_thickness).copied(),
        thickest: found.iter().max_by(by_thickness).copied(),
        mean,
        p05: at_share(&sorted, 0.05),
        median: at_share(&sorted, 0.5),
        p95: at_share(&sorted, 0.95),
        thin,
        share_below,
        probes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::cuboid;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3::new(x, y, z)
    }

    fn middle(positions: &[Vec3]) -> Vec3 {
        positions.iter().fold(Vec3::ZERO, |a, p| a + *p) / positions.len() as f64
    }

    fn bounds(positions: &[Vec3]) -> (Vec3, Vec3) {
        positions.iter().fold((v(1e9, 1e9, 1e9), v(-1e9, -1e9, -1e9)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
    }

    #[test]
    fn three_faces_of_a_tilted_box_put_it_in_the_positive_corner() {
        let (positions, _) = cuboid(v(10.0, 20.0, 30.0), v(60.0, 50.0, 40.0));
        let turn = Rigid::turn(v(1.0, 0.0, 0.0), 0.2).then(&Rigid::turn(v(0.0, 0.0, 1.0), 0.5)).then(&Rigid::shift(v(5.0, -7.0, 3.0)));
        let planes = [
            DatumPlane { point: turn.apply(v(30.0, 30.0, 30.0)), normal: turn.turned(v(0.0, 0.0, -1.0)) },
            DatumPlane { point: turn.apply(v(10.0, 30.0, 35.0)), normal: turn.turned(v(-1.0, 0.0, 0.0)) },
            DatumPlane { point: turn.apply(v(30.0, 20.0, 35.0)), normal: turn.turned(v(0.0, -1.0, 0.0)) },
        ];
        let positions: Vec<Vec3> = positions.iter().map(|p| turn.apply(*p)).collect();
        let found = datum(&planes, middle(&positions)).expect("a frame");
        assert!(found.off_square < 1e-6 && found.third_inside, "{found:?}");
        let placed: Vec<Vec3> = positions.iter().map(|p| found.moved.apply(*p)).collect();
        let (lo, hi) = bounds(&placed);
        for (got, want) in [(lo, v(0.0, 0.0, 0.0)), (hi, v(50.0, 30.0, 10.0))] {
            assert!(got.dist(want) < 1e-9, "{got:?} against {want:?}");
        }
    }

    #[test]
    fn a_third_plane_on_the_far_side_leaves_the_part_at_negative_y() {
        let (positions, _) = cuboid(v(0.0, 0.0, 0.0), v(50.0, 30.0, 10.0));
        let planes = [
            DatumPlane { point: v(25.0, 15.0, 0.0), normal: v(0.0, 0.0, 1.0) },
            DatumPlane { point: v(0.0, 15.0, 5.0), normal: v(1.0, 0.0, 0.0) },
            DatumPlane { point: v(25.0, 30.0, 5.0), normal: v(0.0, 1.0, 0.0) },
        ];
        let found = datum(&planes, middle(&positions)).expect("a frame");
        assert!(!found.third_inside, "{found:?}");
        let placed: Vec<Vec3> = positions.iter().map(|p| found.moved.apply(*p)).collect();
        let (lo, hi) = bounds(&placed);
        assert!(lo.dist(v(0.0, -30.0, 0.0)) < 1e-9 && hi.dist(v(50.0, 0.0, 10.0)) < 1e-9, "{lo:?} {hi:?}");
    }

    #[test]
    fn planes_that_do_not_make_a_corner_are_refused() {
        let flat = |z| DatumPlane { point: v(0.0, 0.0, z), normal: v(0.0, 0.0, 1.0) };
        let side = DatumPlane { point: v(0.0, 0.0, 0.0), normal: v(1.0, 0.0, 0.0) };
        assert_eq!(datum(&[flat(0.0), flat(5.0), side], v(1.0, 1.0, 1.0)), Err(MetrologyError::NearlyParallel(0, 1)));
        // All three planes contain the Y axis.
        let tilted = DatumPlane { point: v(0.0, 0.0, 0.0), normal: v(1.0, 0.0, 1.0) };
        assert_eq!(datum(&[flat(0.0), side, tilted], v(1.0, 1.0, 1.0)), Err(MetrologyError::NoCorner));
        let bad = DatumPlane { point: v(0.0, 0.0, 0.0), normal: Vec3::ZERO };
        assert_eq!(datum(&[flat(0.0), side, bad], v(1.0, 1.0, 1.0)), Err(MetrologyError::BadPlane(2)));
    }

    #[test]
    fn faces_a_degree_off_square_are_reported() {
        let one = 1.0f64.to_radians();
        let planes = [
            DatumPlane { point: v(0.0, 0.0, 0.0), normal: v(0.0, 0.0, 1.0) },
            DatumPlane { point: v(0.0, 0.0, 0.0), normal: v(one.cos(), 0.0, -one.sin()) },
            DatumPlane { point: v(0.0, 0.0, 0.0), normal: v(0.0, 1.0, 0.0) },
        ];
        let found = datum(&planes, v(5.0, 5.0, 5.0)).expect("a frame");
        assert!((found.off_square - 1.0).abs() < 1e-6, "{found:?}");
    }

    #[test]
    fn a_plate_reads_as_thick_as_it_is() {
        let (positions, triangles) = cuboid(v(0.0, 0.0, 0.0), v(100.0, 100.0, 5.0));
        let scan = Scan::new(&positions, &triangles);
        let found = wall(&scan, &WallSettings { samples: 400, up_to: Some(20.0), below: Some(6.0), ..Default::default() }).expect("a reading");
        // Rays across the 100 mm of the plate's edges are no wall at 20 mm; the faces read 5 mm.
        assert!(found.measured > 300, "{found:?}");
        assert!((found.median - 5.0).abs() < 1e-6 && (found.p05 - 5.0).abs() < 1e-6 && (found.p95 - 5.0).abs() < 1e-6, "{found:?}");
        assert!((found.thinnest.map_or(0.0, |r| r.thickness) - 5.0).abs() < 1e-6, "{found:?}");
        assert_eq!(found.looking_back, 0, "{found:?}");
        assert!(!found.thin.is_empty() && found.share_below == Some(1.0), "{found:?}");
    }

    #[test]
    fn a_probe_reads_the_wall_it_is_on_and_a_wound_inside_out_scan_says_so() {
        let (positions, triangles) = cuboid(v(0.0, 0.0, 0.0), v(100.0, 40.0, 5.0));
        let scan = Scan::new(&positions, &triangles);
        let top = WallSettings { probes: vec![v(30.0, 20.0, 5.0), v(30.0, 20.0, 0.0), v(100.0, 20.0, 2.0)], ..Default::default() };
        let found = wall(&scan, &top).expect("a reading");
        let thickness: Vec<f64> = found.probes.iter().map(|r| r.map_or(f64::NAN, |r| r.thickness)).collect();
        assert!((thickness[0] - 5.0).abs() < 1e-6 && (thickness[1] - 5.0).abs() < 1e-6 && (thickness[2] - 100.0).abs() < 1e-6, "{thickness:?}");
        assert!((found.probes[0].map_or(0.0, |r| r.across.z)).abs() < 1e-6, "the ray from the top face leaves through the bottom");
        // Wound the other way, the rays go outward and meet no wall, or a face looking back.
        let inside_out: Vec<[u32; 3]> = triangles.iter().map(|t| [t[0], t[2], t[1]]).collect();
        let scan = Scan::new(&positions, &inside_out);
        let wrong = wall(&scan, &top).expect("readings");
        assert!(wrong.probes.iter().all(|r| r.is_err()), "{wrong:?}");
        let right = wall(&scan, &WallSettings { flip: true, ..top }).expect("readings");
        assert!(right.probes.iter().all(|r| r.is_ok()), "{right:?}");
    }

    #[test]
    fn nothing_to_measure_is_refused() {
        let (positions, triangles) = cuboid(v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0));
        let scan = Scan::new(&positions, &triangles);
        assert_eq!(wall(&scan, &WallSettings::default()), Err(MetrologyError::NothingAsked));
    }
}
