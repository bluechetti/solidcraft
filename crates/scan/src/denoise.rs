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

/// What smoothing would do for a scan, and what was done.
#[derive(Clone, Debug, PartialEq)]
pub struct Denoised {
    /// The smoothed positions, one per position of the scan.
    pub positions: Vec<Vec3>,
    /// The strength used, in passes (fractions blend between whole passes), and the one suggested.
    pub strength: f64,
    pub suggested: f64,
    /// The roughness before and after, in mm.
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
        let faces = self.faces_at(positions);
        let mut residuals: Vec<f64> = (0..positions.len())
            .filter(|&v| !self.fixed[v])
            .filter_map(|v| {
                let ring = slice(&self.ring_start, &self.ring, v);
                if ring.len() < 3 {
                    return None;
                }
                let normal = slice(&self.face_start, &self.faces, v)
                    .iter()
                    .fold(Vec3::ZERO, |s, &f| s + faces[f as usize].0 * faces[f as usize].2)
                    .normalized()?;
                Some(ring.iter().map(|&q| normal.dot(positions[q as usize] - positions[v])).sum::<f64>() / ring.len() as f64)
            })
            .collect();
        if residuals.is_empty() {
            return 0.0;
        }
        residuals.sort_by(f64::total_cmp);
        let median = residuals[residuals.len() / 2];
        let mut deviations: Vec<f64> = residuals.iter().map(|r| (r - median).abs()).collect();
        deviations.sort_by(f64::total_cmp);
        MAD_TO_SIGMA * deviations[deviations.len() / 2] / (1.0 + 1.0 / self.mean_degree).sqrt()
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

/// `scan` smoothed at `strength` passes, or at the strength the study suggests.
pub fn denoise(scan: &Scan, strength: Option<f64>) -> Option<Denoised> {
    let mesh = Mesh::new(scan);
    if mesh.triangles.is_empty() {
        return None;
    }
    let edge = typical_edge(scan);
    let original: Vec<Vec3> =
        scan.positions.iter().enumerate().map(|(v, p)| scan.positions.get(mesh.weld[v] as usize).copied().unwrap_or(*p)).collect();
    let moved = |p: &[Vec3]| -> (f64, f64) {
        let (mut squares, mut most, mut count) = (0.0, 0.0f64, 0usize);
        for v in (0..p.len()).filter(|&v| mesh.weld[v] as usize == v) {
            let d = p[v].dist(original[v]);
            squares += d * d;
            most = most.max(d);
            count += 1;
        }
        ((squares / count.max(1) as f64).sqrt(), most)
    };
    // The study: the roughness left and the distance moved after each pass.
    let mut stages = vec![original.clone()];
    let mut rough = vec![mesh.roughness(&original)];
    let mut deviation = vec![0.0];
    for _ in 0..MOST_PASSES {
        let next = mesh.pass(stages.last()?, edge);
        rough.push(mesh.roughness(&next));
        deviation.push(moved(&next).0);
        stages.push(next);
    }
    let tail = MOST_PASSES / 2;
    let shrinkage = ((deviation[MOST_PASSES] - deviation[tail]) / (MOST_PASSES - tail) as f64).max(0.0);
    let error: Vec<f64> = (0..=MOST_PASSES).map(|k| rough[k].hypot(k as f64 * shrinkage)).collect();
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
    let strength = strength.unwrap_or(suggested).clamp(0.0, MOST_PASSES as f64);
    let lo = (strength.floor() as usize).min(MOST_PASSES - 1);
    let f = strength - lo as f64;
    let positions: Vec<Vec3> = (0..original.len())
        .map(|v| {
            let w = mesh.weld[v] as usize;
            let (a, b) = (stages[lo][w], stages[lo + 1][w]);
            a + (b - a) * f
        })
        .collect();
    let (rms_moved, max_moved) = moved(&positions);
    Some(Denoised {
        roughness: mesh.roughness(&positions),
        noise: rough[0],
        positions,
        strength,
        suggested,
        rms_moved,
        max_moved,
        typical_edge: edge,
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
        assert!(done.suggested > 0.0, "smoothing helps a noisy scan: {done:?}");
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

    /// A clean scan is left alone: nothing to gain.
    #[test]
    fn a_clean_scan_is_left_alone() {
        let (positions, triangles) = fine_cuboid(Vec3::ZERO, Vec3::new(30.0, 20.0, 10.0), 10);
        let done = denoise(&Scan::new(&positions, &triangles), None).unwrap();
        assert_eq!(done.suggested, 0.0);
        assert!(done.max_moved < 1e-9, "{}", done.max_moved);
    }
}
