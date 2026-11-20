//! A first look at a scan: how big it is and how it lies, how fine and how noisy it is, what is
//! wrong with it (holes, scraps not joined to the part, edges no hole closes) and how much of it is
//! broad flat faces. Everything here reads the scan once or on a sample, so it stays quick on the
//! biggest scans; it is what a person or an agent looks at before choosing the next tool.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::base::{self, BaseKind};
use crate::{Scan, denoise, holes, pieces, place, remesh};

/// How many places the noise is read at.
const PATCHES: usize = 8;
/// About how many triangles each of those places takes.
const PATCH_TRIANGLES: f64 = 12_000.0;
/// The most triangles one place keeps (a cube that cuts through folded surface takes more).
const PATCH_MOST: usize = 40_000;
/// A place with fewer triangles than this is not read.
const PATCH_LEAST: usize = 200;
/// A noise read off the neighbours with fewer than this share of a place's points read (the rest
/// at what reads as a sharp edge) is checked against a quadric's ([`noise_places`]). Read exactly,
/// a part has 98 % or more of its points read; with the noise tilting faces as far as an edge
/// does, 58 % (read 0.73 of the noise) down to 6 % (read a quarter of it).
const READ_ENOUGH: f64 = 0.75;
/// The quadric's patches reach this many typical edges (some 80 triangles).
const QUADRIC_EDGES: f64 = 3.0;
/// The most points sampled for the scan's size along its own directions.
const SIZE_SAMPLE: usize = 200_000;

/// What a first look at a scan finds.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub triangle_count: usize,
    pub points: usize,
    /// Triangles naming a missing or non-finite position.
    pub unusable: usize,
    /// The box round the scan, in the design's directions.
    pub low: Vec3,
    pub high: Vec3,
    /// The middle of its surface.
    pub centre: Vec3,
    /// Its size along its own directions, longest first (mm).
    pub own_size: [f64; 3],
    /// Its surface area (mm²).
    pub area: f64,
    /// The median triangle side (mm).
    pub typical_edge: f64,
    /// The noise (how far points stand off the surface their neighbours make, robustly) and the
    /// Denoise strength suggested (0: leave it), each the median of the places read; `None` with
    /// no place to read.
    pub noise: Option<f64>,
    pub smoothing: Option<f64>,
    /// How many places the noise was read at.
    pub noise_places: usize,
    /// Rings of open edges: the pieces' outer rims, the holes inside them, the longest hole's
    /// length (mm), and open edges in no ring (pinches, edges of three triangles).
    pub rims: usize,
    pub holes: usize,
    pub longest_hole: f64,
    pub open_edges: usize,
    /// Open edges Repair Mesh closes (cracks between points a hair apart) and those it leaves in
    /// no ring.
    pub tangled_edges: usize,
    pub crack_edges: usize,
    /// Faces that pass through another face (folds, overlapping passes): Repair Mesh takes them
    /// out and fills round them, and a fit or a Boolean on them goes wrong.
    pub crossing_faces: usize,
    /// The pieces not joined to each other, those that float (far smaller than the largest, see
    /// [`pieces::Pieces::floating`]) and their share of the scan's area.
    pub pieces: usize,
    pub floating: usize,
    pub floating_share: f64,
    /// The share of the surface in broad flat faces (what Stand on Base finds).
    pub flat_share: f64,
}

/// A first look at `scan`; pieces smaller than `floating` of the largest's triangles float.
/// `None` for a scan with no usable triangle.
pub fn look(scan: &Scan, floating: f64) -> Option<Look> {
    let (low, high) = scan.bounds()?;
    let unusable = (0..scan.triangle_count()).filter(|&t| scan.corners(t).is_none()).count();
    let area: f64 =
        (0..scan.triangle_count()).filter_map(|t| scan.corners(t)).map(|[a, b, c]| (b - a).cross(c - a).len() / 2.0).filter(|a| a.is_finite()).sum();
    if unusable == scan.triangle_count() || area <= 0.0 {
        return None;
    }
    let centre = crate::surface_centre(scan)?;
    let own_size = own_size(scan, low, high);
    let typical_edge = remesh::typical_edge(scan);
    let read = noise_places(scan, area);
    let middle = |mut v: Vec<f64>| -> Option<f64> {
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied()
    };
    let noise = middle(read.iter().map(|r| r.0).collect());
    let smoothing = middle(read.iter().map(|r| r.1).collect());

    // Holes and rims are read as Repair Mesh leaves the scan: a crack between a file's two copies
    // of one point is no hole, but edges to repair.
    let open = |s: &holes::Survey| s.rings.iter().map(|r| r.points.len()).sum::<usize>() + s.unfillable_edges;
    let (hair, _) = crate::welded_within(scan.positions);
    let rings = holes::survey_with(scan, &hair);
    let crack_edges = open(&holes::survey(scan)).saturating_sub(open(&rings));
    let hole_rings: Vec<&holes::Ring> = rings.rings.iter().filter(|r| !r.rim).collect();
    let open_edges = open(&rings) + crack_edges;
    let found = pieces::pieces(scan);
    let floats = found.floating(floating);
    let floating_area: f64 = found.areas.iter().zip(&floats).filter(|(_, f)| **f).map(|(a, _)| a).sum();
    let flat_share = base::bases(scan).iter().filter(|b| b.kind == BaseKind::Flat).map(|b| b.share).sum::<f64>().min(1.0);

    Some(Look {
        triangle_count: scan.triangle_count(),
        points: scan.positions.len(),
        unusable,
        low,
        high,
        centre,
        own_size,
        area,
        typical_edge,
        noise,
        smoothing,
        noise_places: read.len(),
        rims: rings.rings.len() - hole_rings.len(),
        holes: hole_rings.len(),
        longest_hole: hole_rings.iter().map(|r| r.perimeter).fold(0.0, f64::max),
        open_edges,
        tangled_edges: rings.unfillable_edges + crack_edges,
        crack_edges,
        crossing_faces: crate::repair::crossing_count_welded(scan, &hair),
        pieces: found.sizes.len(),
        floating: floats.iter().filter(|f| **f).count(),
        floating_share: floating_area / area,
        flat_share,
    })
}

/// The scan's noise alone (what [`look`] reads as `noise`), in mm: the median of the places read,
/// `None` with too few triangles at any place to read it.
pub fn noise(scan: &Scan) -> Option<f64> {
    let area: f64 =
        (0..scan.triangle_count()).filter_map(|t| scan.corners(t)).map(|[a, b, c]| (b - a).cross(c - a).len() / 2.0).filter(|a| a.is_finite()).sum();
    if !(area > 0.0) {
        return None;
    }
    let mut read: Vec<f64> = noise_places(scan, area).into_iter().map(|r| r.0).collect();
    read.sort_by(f64::total_cmp);
    read.get(read.len() / 2).copied()
}

/// The scan's size along its own directions, longest first: the box round a sample of its points
/// laid along them as Quick Align lays it; the design's box when it has no such directions.
fn own_size(scan: &Scan, low: Vec3, high: Vec3) -> [f64; 3] {
    let boxed = |lo: Vec3, hi: Vec3| {
        let d = hi - lo;
        let mut s = [d.x, d.y, d.z];
        s.sort_by(|a, b| b.total_cmp(a));
        s
    };
    let Some(lay) = place::quick_align(scan) else { return boxed(low, high) };
    let step = (scan.positions.len() / SIZE_SAMPLE).max(1);
    let mut laid = scan.points().step_by(step).map(|p| lay.apply(p));
    let Some(first) = laid.next() else { return boxed(low, high) };
    let (lo, hi) = laid.fold((first, first), |(lo, hi), p| (lo.min(p), hi.max(p)));
    boxed(lo, hi)
}

/// The noise (read off sharp edges, [`denoise::noise_off_creases`]) and the suggested Denoise
/// strength at up to [`PATCHES`] places spread over the scan: each place the triangles whose middles
/// lie in a cube round an evenly picked triangle, sized to take about [`PATCH_TRIANGLES`] of them.
/// A place with no vertex off a sharp edge is not read.
fn noise_places(scan: &Scan, area: f64) -> Vec<(f64, f64)> {
    let count = scan.triangle_count();
    let usable = (0..count).filter(|&t| scan.corners(t).is_some()).count().max(1);
    let half = (PATCH_TRIANGLES * area / usable as f64).sqrt() / 2.0;
    if !(half.is_finite() && half > 0.0) {
        return Vec::new();
    }
    let middle = |t: usize| scan.corners(t).map(|[a, b, c]| (a + b + c) / 3.0);
    let seeds: Vec<Vec3> = (0..PATCHES).filter_map(|k| middle((2 * k + 1) * count / (2 * PATCHES))).collect();
    let mut taken: Vec<Vec<usize>> = vec![Vec::new(); seeds.len()];
    for t in 0..count {
        let Some(m) = middle(t) else { continue };
        for (seed, list) in seeds.iter().zip(taken.iter_mut()) {
            let d = m - *seed;
            if list.len() < PATCH_MOST && d.x.abs() <= half && d.y.abs() <= half && d.z.abs() <= half {
                list.push(t);
            }
        }
    }
    taken
        .iter()
        .filter(|list| list.len() >= PATCH_LEAST)
        .filter_map(|list| {
            let (positions, triangles) = piece_of(scan, list);
            let piece = Scan::new(&positions, &triangles);
            let study = denoise::study(&piece)?;
            let (noise, read) = denoise::noise_off_creases_read(&piece)?;
            // Read off its neighbours, a point's noise is exact while it is small against the
            // triangles (every point read), and falls behind once the noise tilts the faces round
            // a point as far as a sharp edge does: the noisiest points are passed over as edges
            // (0.3 mm on 0.5 mm triangles read 0.15, with a quarter of the points read). Where
            // fewer than [`READ_ENOUGH`] were read, the quadric fitted to a few dozen points round
            // each place is read too (Find All's reading), and the larger kept.
            let edge = remesh::typical_edge(&piece);
            let noise = if read < READ_ENOUGH && edge > 0.0 {
                crate::segment::noise_within(&piece, QUADRIC_EDGES * edge).map_or(noise, |q| q.max(noise))
            } else {
                noise
            };
            (noise.is_finite() && study.suggested.is_finite()).then_some((noise, study.suggested))
        })
        .collect()
}

/// The triangles `list` of `scan` as a mesh of their own.
pub(crate) fn piece_of(scan: &Scan, list: &[usize]) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    let mut index = std::collections::HashMap::new();
    let mut positions = Vec::new();
    let mut triangles = Vec::with_capacity(list.len());
    for &t in list {
        let (Some(tri), Some(corners)) = (scan.triangles.get(t), scan.corners(t)) else { continue };
        let mut out = [0u32; 3];
        for ((slot, &v), p) in out.iter_mut().zip(tri).zip(corners) {
            *slot = *index.entry(v).or_insert_with(|| {
                positions.push(p);
                (positions.len() - 1) as u32
            });
        }
        triangles.push(out);
    }
    (positions, triangles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A clean closed box: its sizes and area as built, no holes, one piece, no noise to smooth,
    /// most of it flat.
    #[test]
    fn a_clean_box_reads_as_built() {
        let (p, t) = fine_cuboid(Vec3::ZERO, Vec3::new(40.0, 20.0, 10.0), 24);
        let look = look(&Scan::new(&p, &t), 0.02).unwrap();
        assert_eq!((look.triangle_count, look.unusable, look.holes, look.pieces, look.floating), (t.len(), 0, 0, 1, 0));
        assert_eq!(look.open_edges, 0);
        assert!((look.area - 2.0 * (800.0 + 400.0 + 200.0)).abs() < 1e-6, "{}", look.area);
        for (got, want) in look.own_size.iter().zip([40.0, 20.0, 10.0]) {
            assert!((got - want).abs() < 1e-6, "{:?}", look.own_size);
        }
        assert!(look.noise.is_some_and(|n| n < 1e-6), "{:?}", look.noise);
        assert_eq!(look.smoothing, Some(0.0));
        assert!(look.noise_places >= 1);
        assert!(look.flat_share > 0.5, "{}", look.flat_share);
    }

    /// A box with a hole cut in one side, a speck beside it and noise on every point: the hole,
    /// the speck and the noise are all found.
    #[test]
    fn holes_specks_and_noise_are_found() {
        let (mut p, t) = fine_cuboid(Vec3::ZERO, Vec3::new(40.0, 40.0, 40.0), 30);
        // The two triangles of one square in the middle of the first side left out: a hole.
        let mut t: Vec<[u32; 3]> = t.into_iter().enumerate().filter(|(i, _)| !(930..932).contains(i)).map(|(_, t)| t).collect();
        let n = p.len() as u32;
        p.extend([Vec3::new(90.0, 0.0, 0.0), Vec3::new(91.0, 0.0, 0.0), Vec3::new(90.0, 1.0, 0.0)]);
        t.push([n, n + 1, n + 2]);
        let clean = look(&Scan::new(&p, &t), 0.02).unwrap();
        assert_eq!((clean.holes, clean.pieces, clean.floating), (1, 2, 1), "{clean:?}");
        assert!(clean.longest_hole > 0.0 && clean.floating_share < 0.01);
        // Noise of about 0.05 mm, the same every run.
        let mut seed = 12345u64;
        let noisy: Vec<Vec3> = p
            .iter()
            .map(|q| {
                let mut r = || {
                    seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 0.17
                };
                Vec3::new(q.x + r(), q.y + r(), q.z + r())
            })
            .collect();
        let rough = look(&Scan::new(&noisy, &t), 0.02).unwrap();
        let noise = rough.noise.unwrap();
        assert!(noise > 0.01 && noise < 0.2, "{noise}");
        assert_eq!(super::noise(&Scan::new(&noisy, &t)), Some(noise), "the noise alone reads the same");
        assert!(rough.smoothing.is_some_and(|s| s > 0.0), "{rough:?}");
    }

    /// A clean CAD mesh of a plate with a round hole through it, every vertex on a rim or a corner
    /// and the faces fanned between them: no noise. Read at every vertex, the sharp edges read as
    /// noise (here 0.06 mm; 1 mm on the corpus's holes.stl, which set Fit Plane's tolerance to 3 mm).
    #[test]
    fn a_cad_plate_with_a_hole_reads_no_noise() {
        let (p, t) = crate::test_meshes::holed_plate(40.0, 5.0, 8.0, 48);
        let scan = Scan::new(&p, &t);
        let at_every_vertex = denoise::study(&scan).unwrap().stages[0].roughness;
        assert!(at_every_vertex > 0.04, "the case reads its edges as noise when every vertex counts: {at_every_vertex}");
        let look = look(&scan, 0.02).unwrap();
        assert!(look.noise.is_none_or(|n| n < 0.01), "{:?}", look.noise);
        assert_eq!(super::noise(&scan), look.noise);
    }

    /// Nothing to look at is no look, not a panic.
    #[test]
    fn an_empty_or_broken_scan_is_none() {
        assert!(look(&Scan::new(&[], &[]), 0.02).is_none());
        let p = [Vec3::new(f64::NAN, 0.0, 0.0), Vec3::X, Vec3::Y];
        assert!(look(&Scan::new(&p, &[[0, 1, 2], [0, 7, 9]]), 0.02).is_none());
    }
}
