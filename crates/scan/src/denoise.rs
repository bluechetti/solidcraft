//! Denoise: a scan's noise smoothed away while its sharp edges stay sharp. After SurfaceCraft's
//! denoise study, with the fault its scorecard found put right (its vertex filter rounded a part's
//! edges at low noise): each pass filters the faces' normals bilaterally (a face's neighbours count
//! by how near they are and how alike they face, so faces across a sharp edge barely mix; Zheng et
//! al. 2011) and then moves the vertices until the faces lie square to their filtered normals (Sun
//! et al. 2007). The open rim of a scan stays put.
//!
//! How hard to smooth is measured: the roughness left after each pass (how far a vertex's
//! neighbours stand off its tangent plane, robustly) and how far the surface has moved, whose
//! steady growth once the noise is gone is the shrinkage a pass costs. The error a strength leaves
//! is modelled as the two together, and the strength with the least is suggested, when it clearly
//! beats leaving the scan alone; between whole passes the positions are blended.

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::{Scan, par, welded};

/// The most passes studied.
pub const MOST_PASSES: usize = 8;
/// How far apart two faces' normals may be (radians) and still count alike in the filter.
const NORMAL_SIGMA: f64 = 0.35;
/// Vertex moves after each filtering of the normals.
const VERTEX_STEPS: usize = 8;
/// The suggested strength must lower the modelled error by at least this share.
const REQUIRED_IMPROVEMENT: f64 = 0.10;
/// The median absolute deviation as a standard deviation, for normal noise.
const MAD_TO_SIGMA: f64 = 1.4826;
/// A vertex with a face turned further than this from its normal (radians) is on a sharp edge,
/// where the noise is not read.
const CREASE: f64 = std::f64::consts::FRAC_PI_4;

/// What smoothing would do for a scan, and what was done.
#[derive(Clone, Debug, PartialEq)]
pub struct Denoised {
    /// The smoothed positions, one per position of the scan.
    pub positions: Vec<Vec3>,
    /// The strength used, in passes (fractions blend between whole passes), and the one the
    /// study suggested when it was asked for the strength.
    pub strength: f64,
    pub suggested: Option<f64>,
    /// The noise before and after, in mm, read off the sharp edges ([`Stage::noise`]).
    pub noise: f64,
    pub roughness: f64,
    /// How far the vertices moved: root mean square and largest, in mm.
    pub rms_moved: f64,
    pub max_moved: f64,
    /// The typical edge, in mm.
    pub typical_edge: f64,
}

/// A scan's welded mesh as the filter reads it.
struct Mesh {
    /// Each position's first copy at the same place.
    weld: Vec<u32>,
    /// The triangles on welded positions, those gone to a line or a point left out.
    triangles: Vec<[u32; 3]>,
    /// Each position's triangles and neighbouring positions, packed.
    face_start: Vec<u32>,
    faces: Vec<u32>,
    ring_start: Vec<u32>,
    ring: Vec<u32>,
    /// Positions on the open rim (or not their own representative): never moved.
    fixed: Vec<bool>,
    mean_degree: f64,
    /// Each triangle's neighbours: those sharing a corner with it.
    beside_start: Vec<u32>,
    beside: Vec<u32>,
}

fn packed(lists: &[Vec<u32>]) -> (Vec<u32>, Vec<u32>) {
    let mut start = Vec::with_capacity(lists.len() + 1);
    let mut flat = Vec::new();
    start.push(0);
    for l in lists {
        flat.extend_from_slice(l);
        start.push(flat.len() as u32);
    }
    (start, flat)
}

fn slice<'a>(start: &[u32], flat: &'a [u32], i: usize) -> &'a [u32] {
    let (a, b) = (start.get(i).copied().unwrap_or(0) as usize, start.get(i + 1).copied().unwrap_or(0) as usize);
    flat.get(a..b).unwrap_or_default()
}

/// Whether two of `normals` are the same to the last few bits: two faces round a vertex on one
/// exact plane, as a CAD mesh is drawn (or a region Flatten laid flat); measured points never lie so.
fn drawn(normals: impl Iterator<Item = Vec3>) -> bool {
    let all: Vec<Vec3> = normals.filter(|n| *n != Vec3::ZERO).collect();
    all.iter().enumerate().any(|(i, a)| all[i + 1..].iter().any(|b| a.dist(*b) < 1e-9))
}

impl Mesh {
    fn new(scan: &Scan) -> Self {
        let n = scan.positions.len();
        let weld = welded(scan.positions);
        let triangles: Vec<[u32; 3]> = (0..scan.triangle_count())
            .filter_map(|t| {
                scan.corners(t)?;
                let [a, b, c] = scan.triangles[t].map(|v| weld[v as usize]);
                (a != b && b != c && c != a).then_some([a, b, c])
            })
            .collect();
        let mut faces_of: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut ring_of: Vec<Vec<u32>> = vec![Vec::new(); n];
        let mut uses: std::collections::HashMap<(u32, u32), u32> = std::collections::HashMap::new();
        for (f, t) in triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                faces_of[a as usize].push(f as u32);
                ring_of[a as usize].push(b);
                ring_of[b as usize].push(a);
                *uses.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let mut fixed: Vec<bool> = (0..n).map(|v| weld[v] as usize != v).collect();
        for ((a, b), count) in uses {
            if count == 1 {
                fixed[a as usize] = true;
                fixed[b as usize] = true;
            }
        }
        for r in &mut ring_of {
            r.sort_unstable();
            r.dedup();
        }
        let interior: Vec<usize> = (0..n).filter(|&v| !fixed[v] && ring_of[v].len() >= 3).map(|v| ring_of[v].len()).collect();
        let mean_degree = if interior.is_empty() { 6.0 } else { interior.iter().sum::<usize>() as f64 / interior.len() as f64 };
        let beside_of: Vec<Vec<u32>> = triangles
            .iter()
            .enumerate()
            .map(|(f, t)| {
                let mut b: Vec<u32> = t.iter().flat_map(|&v| faces_of[v as usize].iter().copied()).filter(|&g| g as usize != f).collect();
                b.sort_unstable();
                b.dedup();
                b
            })
            .collect();
        let (face_start, faces) = packed(&faces_of);
        let (ring_start, ring) = packed(&ring_of);
        let (beside_start, beside) = packed(&beside_of);
        Mesh { weld, triangles, face_start, faces, ring_start, ring, fixed, mean_degree, beside_start, beside }
    }

    /// Each triangle's unit normal (zero for none), middle and area.
    fn faces_at(&self, positions: &[Vec3]) -> Vec<(Vec3, Vec3, f64)> {
        self.triangles
            .iter()
            .map(|t| {
                let [a, b, c] = t.map(|v| positions[v as usize]);
                let cross = (b - a).cross(c - a);
                (cross.normalized().unwrap_or(Vec3::ZERO), (a + b + c) / 3.0, cross.len() / 2.0)
            })
            .collect()
    }

    /// The roughness of `positions`: the robust spread of each vertex's neighbours' mean height
    /// over its tangent plane, less the gain of averaging over them.
    fn roughness(&self, positions: &[Vec3]) -> f64 {
        self.roughness_where(positions, false).0
    }

    /// The roughness of `positions` and how many vertices it was read at. With `off_creases`, only
    /// vertices whose faces all lie within [`CREASE`] of their normal are read: at a vertex on a
    /// sharp edge the neighbours stand off the tangent plane by the part's own shape, not noise (a
    /// CAD mesh has almost no other vertices). A vertex with two faces on one exact plane
    /// ([`drawn`]) reads as no noise: on a CAD mesh a rim vertex too gentle for [`CREASE`] (a
    /// countersink's) would read its long triangles' slope as millimetres of noise.
    fn roughness_where(&self, positions: &[Vec3], off_creases: bool) -> (f64, usize) {
        let faces = self.faces_at(positions);
        let crease_cos = CREASE.cos();
        let mut residuals: Vec<f64> = (0..positions.len())
            .filter(|&v| !self.fixed[v])
            .filter_map(|v| {
                let ring = slice(&self.ring_start, &self.ring, v);
                if ring.len() < 3 {
                    return None;
                }
                let around = slice(&self.face_start, &self.faces, v);
                let normal = around.iter().fold(Vec3::ZERO, |s, &f| s + faces[f as usize].0 * faces[f as usize].2).normalized()?;
                if off_creases && around.iter().any(|&f| faces[f as usize].0.dot(normal) < crease_cos) {
                    return None;
                }
                // Drawn exactly, not measured: no noise there.
                if off_creases && drawn(around.iter().map(|&f| faces[f as usize].0)) {
                    return Some(0.0);
                }
                Some(ring.iter().map(|&q| normal.dot(positions[q as usize] - positions[v])).sum::<f64>() / ring.len() as f64)
            })
            .collect();
        if residuals.is_empty() {
            return (0.0, 0);
        }
        residuals.sort_by(f64::total_cmp);
        let median = residuals[residuals.len() / 2];
        let mut deviations: Vec<f64> = residuals.iter().map(|r| (r - median).abs()).collect();
        deviations.sort_by(f64::total_cmp);
        (MAD_TO_SIGMA * deviations[deviations.len() / 2] / (1.0 + 1.0 / self.mean_degree).sqrt(), residuals.len())
    }

    /// One pass: the face normals filtered bilaterally, then the vertices moved to lie square to them.
    fn pass(&self, positions: &[Vec3], spacing: f64) -> Vec<Vec3> {
        let faces = self.faces_at(positions);
        let (two_c, two_s) = (2.0 * spacing * spacing, 2.0 * NORMAL_SIGMA * NORMAL_SIGMA);
        let filtered: Vec<Vec3> = par::map(faces.len(), |f| {
            let (n, c, a) = faces[f];
            let mut sum = n * a;
            for &g in slice(&self.beside_start, &self.beside, f) {
                let (m, d, b) = faces[g as usize];
                // Faces wound the other way are the same surface.
                let m = if m.dot(n) < 0.0 { -m } else { m };
                let wc = (-(c.dist(d).powi(2)) / two_c).exp();
                let ws = (-(n.dist(m).powi(2)) / two_s).exp();
                sum += m * (b * wc * ws);
            }
            sum.normalized().unwrap_or(n)
        });
        let mut next = positions.to_vec();
        for _ in 0..VERTEX_STEPS {
            let middles: Vec<Vec3> = self.triangles.iter().map(|t| t.iter().fold(Vec3::ZERO, |s, &v| s + next[v as usize]) / 3.0).collect();
            next = par::map(next.len(), |v| {
                let p = next[v];
                if self.fixed[v] {
                    return p;
                }
                let around = slice(&self.face_start, &self.faces, v);
                if around.is_empty() {
                    return p;
                }
                let shift = around.iter().fold(Vec3::ZERO, |s, &f| {
                    let n = filtered[f as usize];
                    s + n * n.dot(middles[f as usize] - p)
                });
                p + shift / around.len() as f64
            });
        }
        next
    }
}

/// The middle length of the mesh's edges.
fn typical_edge(scan: &Scan) -> f64 {
    crate::remesh::typical_edge(scan)
}

/// The roughness and the movement after a number of passes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    /// The roughness left, in mm, read at every vertex: what the error a strength leaves is
    /// modelled on.
    pub roughness: f64,
    /// The noise left, in mm, read off the sharp edges ([`noise_off_creases`]): what is shown.
    /// Read at every vertex, a CAD mesh's edges read as millimetres of noise.
    pub noise: f64,
    /// How far the vertices have moved: root mean square and largest, in mm.
    pub rms_moved: f64,
    pub max_moved: f64,
}

/// What smoothing would do for a scan: the roughness and movement after each whole pass, from 0
/// to [`MOST_PASSES`], and the strength suggested (0: the scan is as smooth as it can be read).
#[derive(Clone, Debug, PartialEq)]
pub struct Study {
    pub stages: Vec<Stage>,
    pub suggested: f64,
    /// The noise before (off the sharp edges, [`Stage::noise`]), and the typical edge, in mm.
    pub noise: f64,
    pub typical_edge: f64,
}

impl Study {
    /// The roughness and movement at `strength` passes, blended between the whole passes either
    /// side (an estimate between them; smoothing at it measures it exactly).
    pub fn at(&self, strength: f64) -> Stage {
        let s = strength.clamp(0.0, (self.stages.len() - 1) as f64);
        let lo = (s.floor() as usize).min(self.stages.len().saturating_sub(2));
        let f = s - lo as f64;
        let (a, b) = (self.stages[lo], self.stages[(lo + 1).min(self.stages.len() - 1)]);
        let mix = |x: f64, y: f64| x * (1.0 - f) + y * f;
        Stage {
            roughness: mix(a.roughness, b.roughness),
            noise: mix(a.noise, b.noise),
            rms_moved: mix(a.rms_moved, b.rms_moved),
            max_moved: mix(a.max_moved, b.max_moved),
        }
    }
}

/// The scan's mesh and its first positions, to run passes on.
struct Run {
    mesh: Mesh,
    original: Vec<Vec3>,
    edge: f64,
}

impl Run {
    fn new(scan: &Scan) -> Option<Run> {
        Run::within(scan, None)
    }

    /// A run that moves only the vertices of the triangles `area` marks (one flag a triangle of the
    /// scan), and reads the roughness only there: a vertex of a triangle outside it is held.
    fn within(scan: &Scan, area: Option<&[bool]>) -> Option<Run> {
        let mut mesh = Mesh::new(scan);
        if mesh.triangles.is_empty() {
            return None;
        }
        if let Some(area) = area {
            for (t, tri) in scan.triangles.iter().enumerate() {
                if !area.get(t).copied().unwrap_or(false) {
                    for v in tri {
                        if let Some(rep) = mesh.weld.get(*v as usize).copied()
                            && let Some(f) = mesh.fixed.get_mut(rep as usize)
                        {
                            *f = true;
                        }
                    }
                }
            }
        }
        let original = scan.positions.iter().enumerate().map(|(v, p)| scan.positions.get(mesh.weld[v] as usize).copied().unwrap_or(*p)).collect();
        Some(Run { mesh, original, edge: typical_edge(scan) })
    }

    /// How far `p` stands from the first positions: root mean square and largest.
    fn moved(&self, p: &[Vec3]) -> (f64, f64) {
        let (mut squares, mut most, mut count) = (0.0, 0.0f64, 0usize);
        for v in (0..p.len()).filter(|&v| self.mesh.weld[v] as usize == v) {
            let d = p[v].dist(self.original[v]);
            squares += d * d;
            most = most.max(d);
            count += 1;
        }
        ((squares / count.max(1) as f64).sqrt(), most)
    }

    fn stage(&self, p: &[Vec3]) -> Stage {
        let (rms_moved, max_moved) = self.moved(p);
        Stage { roughness: self.mesh.roughness(p), noise: self.mesh.roughness_where(p, true).0, rms_moved, max_moved }
    }

    /// `p` with every position following its weld (a pass moves only the first copy of each).
    fn expanded(&self, p: &[Vec3]) -> Vec<Vec3> {
        (0..self.original.len()).map(|v| p.get(self.mesh.weld[v] as usize).or(p.get(v)).copied().unwrap_or(self.original[v])).collect()
    }

    /// The positions at `strength` passes: as many whole passes as it reaches, the last two
    /// blended, every position following its weld.
    fn smoothed(&self, strength: f64) -> Option<Vec<Vec3>> {
        let whole = strength.ceil() as usize;
        let (mut before, mut after) = (self.original.clone(), self.original.clone());
        for _ in 0..whole {
            before = after;
            after = self.mesh.pass(&before, self.edge);
        }
        let f = if whole == 0 { 0.0 } else { strength - (whole - 1) as f64 };
        (0..self.original.len())
            .map(|v| {
                let w = self.mesh.weld[v] as usize;
                let (a, b) = (before.get(w).copied()?, after.get(w).copied()?);
                Some(a + (b - a) * f)
            })
            .collect()
    }
}

/// The study: every pass up to [`MOST_PASSES`] run and measured, and the strength suggested. The
/// error a strength leaves is modelled as the roughness and the shrinkage together, the
/// shrinkage being the steady growth of the movement over the last passes, once the noise is gone.
pub fn study(scan: &Scan) -> Option<Study> {
    study_within(scan, None)
}

/// [`study`] of the triangles `area` marks (one flag a triangle of the scan; all with `None`): only
/// they are smoothed and measured.
pub fn study_within(scan: &Scan, area: Option<&[bool]>) -> Option<Study> {
    let run = Run::within(scan, area)?;
    let mut stages = vec![run.stage(&run.original)];
    let mut at = run.original.clone();
    for _ in 0..MOST_PASSES {
        at = run.mesh.pass(&at, run.edge);
        stages.push(run.stage(&at));
    }
    let tail = MOST_PASSES / 2;
    let shrinkage = ((stages[MOST_PASSES].rms_moved - stages[tail].rms_moved) / (MOST_PASSES - tail) as f64).max(0.0);
    let error: Vec<f64> = (0..=MOST_PASSES).map(|k| stages[k].roughness.hypot(k as f64 * shrinkage)).collect();
    let blend = |values: &[f64], s: f64| {
        let lo = (s.floor() as usize).min(MOST_PASSES - 1);
        let f = s - lo as f64;
        values[lo] * (1.0 - f) + values[lo + 1] * f
    };
    let (mut best, mut best_value) = (0.0, error[0]);
    let mut s = 0.0;
    while s <= MOST_PASSES as f64 {
        let value = blend(&error, s);
        if value < best_value {
            (best, best_value) = (s, value);
        }
        s += 0.05;
    }
    let suggested = if best_value <= (1.0 - REQUIRED_IMPROVEMENT) * error[0] { (best * 20.0).round() / 20.0 } else { 0.0 };
    Some(Study { noise: stages[0].noise, stages, suggested, typical_edge: run.edge })
}

/// `scan`'s noise read away from its sharp edges, in mm (0 where it is drawn exactly, as a CAD
/// mesh is); `None` when no vertex lies off one ([`Study::noise`] reads 0 then).
pub fn noise_off_creases(scan: &Scan) -> Option<f64> {
    noise_off_creases_read(scan).map(|(noise, _)| noise)
}

/// [`noise_off_creases`], and the share of the vertices it could read that it did read (the
/// rest lie at what reads as a sharp edge). On a part, sharp edges are lines and take few of its
/// vertices; where most read as sharp, the noise itself tilts the faces that far.
pub fn noise_off_creases_read(scan: &Scan) -> Option<(f64, f64)> {
    let run = Run::new(scan)?;
    let (noise, read) = run.mesh.roughness_where(&run.original, true);
    let free = run.mesh.fixed.iter().filter(|f| !**f).count();
    (read > 0).then(|| (noise, read as f64 / free.max(1) as f64))
}

/// The share of `scan`'s vertices with two faces on one exact plane: drawn (a CAD mesh, or a region
/// Flatten laid flat), not measured as a scanner measures. A real scan has a few such vertices
/// (a thousandth at most in the T-LESS scans); a CAD mesh has most of its vertices so.
pub fn drawn_share(scan: &Scan) -> f64 {
    let mesh = Mesh::new(scan);
    let faces = mesh.faces_at(scan.positions);
    let (mut drawn_count, mut count) = (0usize, 0usize);
    for v in 0..scan.positions.len() {
        let around = slice(&mesh.face_start, &mesh.faces, v);
        if around.is_empty() {
            continue;
        }
        count += 1;
        drawn_count += usize::from(drawn(around.iter().map(|&f| faces[f as usize].0)));
    }
    if count == 0 { 0.0 } else { drawn_count as f64 / count as f64 }
}

/// `scan`'s positions smoothed pass by pass until `enough` finds them smooth enough or `most`
/// passes have run, and how many ran; `None` for a scan with nothing to smooth.
pub(crate) fn smoothed_until(scan: &Scan, most: usize, mut enough: impl FnMut(&[Vec3]) -> bool) -> Option<(Vec<Vec3>, usize)> {
    let run = Run::new(scan)?;
    let mut at = run.original.clone();
    let mut positions = at.clone();
    for pass in 1..=most {
        at = run.mesh.pass(&at, run.edge);
        positions = run.expanded(&at);
        if enough(&positions) {
            return Some((positions, pass));
        }
    }
    Some((positions, most))
}

/// `scan` smoothed at `strength` passes, or at the strength the study suggests (which runs the
/// study first). Given a strength, only the passes it reaches are run.
pub fn denoise(scan: &Scan, strength: Option<f64>) -> Option<Denoised> {
    denoise_within(scan, strength, None)
}

/// [`denoise`] within an area: only the vertices of the triangles `area` marks move (one flag a
/// triangle of the scan; all with `None`), so the rest of the scan and the area's edge with it stay
/// as they were; the noise and roughness are read there.
pub fn denoise_within(scan: &Scan, strength: Option<f64>, area: Option<&[bool]>) -> Option<Denoised> {
    let (strength, suggested) = match strength {
        Some(k) => (k.clamp(0.0, MOST_PASSES as f64), None),
        None => {
            let found = study_within(scan, area)?;
            (found.suggested, Some(found.suggested))
        }
    };
    let run = Run::within(scan, area)?;
    let positions = run.smoothed(strength)?;
    let measured = run.stage(&positions);
    Some(Denoised {
        roughness: measured.noise,
        noise: run.mesh.roughness_where(&run.original, true).0,
        positions,
        strength,
        suggested,
        rms_moved: measured.rms_moved,
        max_moved: measured.max_moved,
        typical_edge: run.edge,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_meshes::fine_cuboid;

    /// A deterministic noise in −½ … ½.
    fn noise(i: usize) -> f64 {
        let s = ((i as f64) * 12.9898).sin() * 43_758.545;
        s - s.floor() - 0.5
    }

    /// The distance of `p` from the surface of the box from 0 to `size`.
    fn off_box(p: Vec3, size: Vec3) -> f64 {
        let inside = |v: f64, s: f64| v.min(s - v);
        let d = [inside(p.x, size.x), inside(p.y, size.y), inside(p.z, size.z)];
        if d.iter().all(|v| *v >= 0.0) {
            d.into_iter().fold(f64::INFINITY, f64::min)
        } else {
            let out = |v: f64, s: f64| (-v).max(v - s).max(0.0);
            Vec3::new(out(p.x, size.x), out(p.y, size.y), out(p.z, size.z)).len()
        }
    }

    /// A box scanned with noise along its faces: smoothed at the suggested strength, it lies nearer
    /// its faces, and its edges are no rounder than they were (the fault the vertex filter had).
    #[test]
    fn a_noisy_box_is_smoothed_and_keeps_its_edges() {
        let size = Vec3::new(30.0, 20.0, 10.0);
        let (mut positions, triangles) = fine_cuboid(Vec3::ZERO, size, 30);
        let welds = welded(&positions);
        for (i, p) in positions.iter_mut().enumerate() {
            // Each place moved alike, so the box stays welded; along the normal of the face it is on.
            let k = welds[i] as usize;
            let n = if p.x.min(size.x - p.x) < 1e-9 {
                Vec3::X
            } else if p.y.min(size.y - p.y) < 1e-9 {
                Vec3::Y
            } else {
                Vec3::Z
            };
            *p += n * (0.06 * noise(k));
        }
        let scan = Scan::new(&positions, &triangles);
        let done = denoise(&scan, None).unwrap();
        assert!(done.suggested.is_some_and(|k| k > 0.0), "smoothing helps a noisy scan: {done:?}");
        assert!(done.roughness < 0.5 * done.noise, "{} from {}", done.roughness, done.noise);
        let rms = |ps: &[Vec3]| (ps.iter().map(|p| off_box(*p, size).powi(2)).sum::<f64>() / ps.len() as f64).sqrt();
        assert!(rms(&done.positions) < 0.7 * rms(&positions), "{} from {}", rms(&done.positions), rms(&positions));
        // At the edges: the vertices within a millimetre of two faces, no further off than before.
        let near_edge = |p: Vec3| [p.x.min(size.x - p.x), p.y.min(size.y - p.y), p.z.min(size.z - p.z)].iter().filter(|d| d.abs() < 1.0).count() >= 2;
        let edge: Vec<usize> = (0..positions.len()).filter(|&i| near_edge(positions[i])).collect();
        let (before, after) =
            (edge.iter().map(|&i| off_box(positions[i], size)).sum::<f64>(), edge.iter().map(|&i| off_box(done.positions[i], size)).sum::<f64>());
        assert!(after <= 1.05 * before, "edges no rounder: {after} against {before}");
    }

    /// Within an area (the box's top), only the area is smoothed: every vertex of a triangle
    /// outside it is exactly where it was, and the top's noise is taken down.
    #[test]
    fn only_the_area_is_smoothed() {
        let size = Vec3::new(30.0, 20.0, 10.0);
        let (mut positions, triangles) = fine_cuboid(Vec3::ZERO, size, 30);
        let welds = welded(&positions);
        for (i, p) in positions.iter_mut().enumerate() {
            let k = welds[i] as usize;
            let n = if p.x.min(size.x - p.x) < 1e-9 {
                Vec3::X
            } else if p.y.min(size.y - p.y) < 1e-9 {
                Vec3::Y
            } else {
                Vec3::Z
            };
            *p += n * (0.06 * noise(k));
        }
        let scan = Scan::new(&positions, &triangles);
        let top = |t: &[u32; 3]| t.iter().all(|&v| positions[v as usize].z > size.z - 0.2);
        let area: Vec<bool> = triangles.iter().map(top).collect();
        let done = denoise_within(&scan, Some(3.0), Some(&area)).unwrap();
        let outside: std::collections::HashSet<u32> = triangles.iter().zip(&area).filter(|(_, a)| !**a).flat_map(|(t, _)| *t).collect();
        for v in &outside {
            let i = *v as usize;
            assert_eq!(done.positions[i], positions[welds[i] as usize], "vertex {v} outside the area moved");
        }
        let moved = (0..positions.len()).filter(|i| !outside.contains(&(*i as u32)) && done.positions[*i] != positions[*i]).count();
        assert!(moved > 100, "the top smoothed: {moved} moved");
        assert!(done.roughness < 0.5 * done.noise, "{} from {}", done.roughness, done.noise);
        let found = study_within(&scan, Some(&area)).unwrap();
        assert!(found.suggested > 0.0, "smoothing helps the noisy top: {found:?}");
    }

    /// A clean scan is left alone: nothing to gain.
    #[test]
    fn a_clean_scan_is_left_alone() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 10);
        let done = denoise(&Scan::new(&positions, &triangles), None).unwrap();
        assert_eq!(done.suggested, Some(0.0));
        assert!(done.max_moved < 1e-9, "{}", done.max_moved);
    }

    /// A clean CAD mesh, every vertex on a rim or a corner: it reads as no noise and is left
    /// alone. Read at every vertex, its edges were shown as noise (about a millimetre on the
    /// corpus's holes.stl).
    #[test]
    fn a_cad_mesh_reads_no_noise() {
        let (positions, triangles) = crate::test_meshes::holed_plate(40.0, 5.0, 8.0, 48);
        let scan = Scan::new(&positions, &triangles);
        let found = study(&scan).unwrap();
        assert!(found.stages[0].roughness > 0.04, "its edges read as roughness: {found:?}");
        assert_eq!((found.noise, found.suggested), (0.0, 0.0), "{found:?}");
        let done = denoise(&scan, None).unwrap();
        assert_eq!((done.noise, done.roughness, done.suggested), (0.0, 0.0, Some(0.0)), "{done:?}");
    }

    /// The study measures every pass: the roughness falls and the movement grows; at a whole pass
    /// it reads what smoothing at that strength measures, and a strength given runs only the
    /// passes it reaches, the same as the study's.
    #[test]
    fn the_study_reads_each_pass_as_smoothing_measures_it() {
        let size = Vec3::new(30.0, 20.0, 10.0);
        let (mut positions, triangles) = fine_cuboid(Vec3::ZERO, size, 24);
        let welds = welded(&positions);
        for (i, p) in positions.iter_mut().enumerate() {
            let k = welds[i] as usize;
            let n = if p.x.min(size.x - p.x) < 1e-9 {
                Vec3::X
            } else if p.y.min(size.y - p.y) < 1e-9 {
                Vec3::Y
            } else {
                Vec3::Z
            };
            *p += n * (0.06 * noise(k));
        }
        let scan = Scan::new(&positions, &triangles);
        let found = study(&scan).unwrap();
        assert_eq!(found.stages.len(), MOST_PASSES + 1);
        assert_eq!(found.stages[0].rms_moved, 0.0);
        assert!(found.stages[1].roughness < found.stages[0].roughness && found.stages[2].rms_moved > found.stages[1].rms_moved, "{found:?}");
        for k in [1.0, 2.0, 3.5] {
            let done = denoise(&scan, Some(k)).unwrap();
            assert_eq!(done.suggested, None);
            let read = found.at(k);
            if k.fract() == 0.0 {
                assert!((read.noise - done.roughness).abs() < 1e-12 && (read.rms_moved - done.rms_moved).abs() < 1e-12, "{k}: {read:?} {done:?}");
            } else {
                assert!((read.rms_moved - done.rms_moved).abs() < 0.2 * done.rms_moved, "an estimate between passes: {read:?} {done:?}");
            }
        }
        assert_eq!(found.at(0.0), found.stages[0]);
        assert_eq!(found.at(99.0), found.stages[MOST_PASSES]);
    }
}
