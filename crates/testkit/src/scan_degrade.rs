//! A clean mesh of a part made into what a scanner gives: even triangles, noise along the
//! surface's normals, areas the scanner did not see, stray specks, an arbitrary pose, another
//! unit. Everything comes from a fixed seed (the crate's xorshift), so a scan-to-CAD benchmark
//! case reproduces exactly, and nothing here uses the scan tools it feeds: plain arithmetic on
//! points and triangles.

use super::Rng;

/// A triangle mesh: points, and triangles as three point indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    pub points: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// What to do to a mesh, in the order [`degraded`] does it. The default changes nothing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Degrade {
    pub seed: u64,
    /// Triangles are cut in four until their edges are no longer than this (0: left as they are).
    pub edge: f64,
    /// Standard deviation of the noise along each point's normal, in the mesh's unit.
    pub sigma: f64,
    /// The share of the surface's area taken away, 0 to 1, as round patches the scanner missed.
    pub missing: f64,
    /// How many patches the missing area is in.
    pub patches: usize,
    /// Stray specks added beside the part, as a share of its triangle count.
    pub specks: f64,
    /// A turn about `axis` by `angle` radians, then a shift.
    pub axis: [f64; 3],
    pub angle: f64,
    pub shift: [f64; 3],
    /// Every coordinate times this, last: 1 / 25.4 writes a millimetre part in inches.
    pub scale: f64,
}

impl Default for Degrade {
    fn default() -> Degrade {
        Degrade { seed: 1, edge: 0.0, sigma: 0.0, missing: 0.0, patches: 3, specks: 0.0, axis: [0.0, 0.0, 1.0], angle: 0.0, shift: [0.0; 3], scale: 1.0 }
    }
}

/// The most times a mesh is cut in four: 4^6 triangles for each one it came with.
const MOST_CUTS: u32 = 6;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn times(a: [f64; 3], k: f64) -> [f64; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}

impl Scan {
    fn corners(&self, t: &[u32; 3]) -> Option<[[f64; 3]; 3]> {
        Some([*self.points.get(t[0] as usize)?, *self.points.get(t[1] as usize)?, *self.points.get(t[2] as usize)?])
    }

    /// The surface's area.
    pub fn area(&self) -> f64 {
        self.triangles.iter().filter_map(|t| self.corners(t)).map(|[a, b, c]| 0.5 * len(cross(sub(b, a), sub(c, a)))).sum()
    }

    /// The longest edge of any triangle.
    pub fn longest_edge(&self) -> f64 {
        self.triangles.iter().filter_map(|t| self.corners(t)).map(|[a, b, c]| len(sub(b, a)).max(len(sub(c, b))).max(len(sub(a, c)))).fold(0.0, f64::max)
    }

    /// Each point's normal: its triangles' normals, the larger triangle counting for more.
    fn normals(&self) -> Vec<[f64; 3]> {
        let mut normals = vec![[0.0; 3]; self.points.len()];
        for t in &self.triangles {
            let Some([a, b, c]) = self.corners(t) else { continue };
            let n = cross(sub(b, a), sub(c, a));
            for &i in t {
                if let Some(slot) = normals.get_mut(i as usize) {
                    *slot = add(*slot, n);
                }
            }
        }
        for n in &mut normals {
            let l = len(*n);
            *n = if l > 0.0 { times(*n, 1.0 / l) } else { [0.0; 3] };
        }
        normals
    }

    /// The mesh as a binary STL file.
    pub fn stl(&self) -> Vec<u8> {
        let kept: Vec<[[f64; 3]; 3]> = self.triangles.iter().filter_map(|t| self.corners(t)).collect();
        let mut out = vec![0u8; 80];
        out.extend((kept.len() as u32).to_le_bytes());
        for corners in kept {
            out.extend([0u8; 12]);
            for p in corners {
                for c in p {
                    out.extend((c as f32).to_le_bytes());
                }
            }
            out.extend([0u8; 2]);
        }
        out
    }
}

/// `mesh` with every triangle cut in four, the cuts shared along each edge, until no edge is
/// longer than `edge` (or [`MOST_CUTS`] times): neighbours still share whole edges.
pub fn even(mesh: &Scan, edge: f64) -> Scan {
    let mut out = mesh.clone();
    let mut cuts = 0;
    while edge > 0.0 && out.longest_edge() > edge && cuts < MOST_CUTS {
        let mut middles: std::collections::BTreeMap<(u32, u32), u32> = std::collections::BTreeMap::new();
        let mut next = Vec::with_capacity(out.triangles.len() * 4);
        let triangles = std::mem::take(&mut out.triangles);
        for [a, b, c] in triangles {
            let mut middle = |i: u32, j: u32, points: &mut Vec<[f64; 3]>| -> u32 {
                *middles.entry((i.min(j), i.max(j))).or_insert_with(|| {
                    let (p, q) = (points.get(i as usize).copied().unwrap_or_default(), points.get(j as usize).copied().unwrap_or_default());
                    points.push(times(add(p, q), 0.5));
                    (points.len() - 1) as u32
                })
            };
            let (ab, bc, ca) = (middle(a, b, &mut out.points), middle(b, c, &mut out.points), middle(c, a, &mut out.points));
            next.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        out.triangles = next;
        cuts += 1;
    }
    out
}

/// A value from the standard normal distribution (Box and Muller's two uniform values).
fn gaussian(rng: &mut Rng) -> f64 {
    let unit = |rng: &mut Rng| ((rng.next() >> 11) as f64 + 1.0) / (1u64 << 53) as f64;
    let (u, v) = (unit(rng), unit(rng));
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

/// `mesh` degraded as `how` says: cut even, patches taken away, noise added, specks added, turned
/// and shifted, scaled. The same `how` on the same mesh gives the same result, bit for bit.
pub fn degraded(mesh: &Scan, how: &Degrade) -> Scan {
    let mut rng = Rng::new(how.seed);
    let mut out = even(mesh, how.edge);
    // Patches the scanner missed: the triangles whose middle is within a patch's reach of a
    // point picked on the surface, the reach chosen so the patches together take the share.
    if how.missing > 0.0 && how.patches > 0 && !out.triangles.is_empty() {
        let reach = (how.missing.min(0.9) * out.area() / (how.patches as f64 * std::f64::consts::PI)).sqrt();
        let middles: Vec<[f64; 3]> = out.triangles.iter().map(|t| out.corners(t).map_or([f64::NAN; 3], |[a, b, c]| times(add(add(a, b), c), 1.0 / 3.0))).collect();
        let centres: Vec<[f64; 3]> = (0..how.patches).filter_map(|_| middles.get(rng.below(middles.len())).copied()).collect();
        let mut keep = middles.iter().map(|m| !centres.iter().any(|c| len(sub(*m, *c)) <= reach));
        out.triangles.retain(|_| keep.next().unwrap_or(true));
    }
    if how.sigma > 0.0 {
        let normals = out.normals();
        for (p, n) in out.points.iter_mut().zip(normals) {
            *p = add(*p, times(n, how.sigma * gaussian(&mut rng)));
        }
    }
    // Specks: small triangles off the surface, each a few edges out along a point's normal.
    let specks = (how.specks.max(0.0) * out.triangles.len() as f64).round() as usize;
    if specks > 0 && !out.points.is_empty() {
        let normals = out.normals();
        let size = if how.edge > 0.0 { how.edge } else { out.longest_edge() };
        for _ in 0..specks {
            let at = rng.below(out.points.len());
            let (Some(&p), Some(&n)) = (out.points.get(at), normals.get(at)) else { continue };
            let base = add(p, times(n, size * (3.0 + (rng.below(1000) as f64) / 100.0)));
            let first = out.points.len() as u32;
            out.points.extend([base, add(base, [size, 0.0, 0.0]), add(base, [0.0, size, 0.0])]);
            out.triangles.push([first, first + 1, first + 2]);
        }
    }
    let l = len(how.axis);
    let axis = if l > 0.0 { times(how.axis, 1.0 / l) } else { [0.0, 0.0, 1.0] };
    let (s, c) = how.angle.sin_cos();
    for p in &mut out.points {
        // Rodrigues' rotation, then the shift, then the unit.
        let turned = add(add(times(*p, c), times(cross(axis, *p), s)), times(axis, dot(axis, *p) * (1.0 - c)));
        *p = times(add(turned, how.shift), how.scale);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A closed box 10 x 10 x 10 of twelve triangles, wound outward.
    fn cube() -> Scan {
        let points = (0..8u32).map(|i| [f64::from(i & 1) * 10.0, f64::from((i >> 1) & 1) * 10.0, f64::from((i >> 2) & 1) * 10.0]).collect();
        let triangles = vec![[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        Scan { points, triangles }
    }

    /// Cut even, a cube keeps its 600 of area, has no edge longer than asked, and stays closed:
    /// every edge is still shared by two triangles.
    #[test]
    fn a_cube_cut_even_keeps_its_area_and_stays_closed() {
        let fine = even(&cube(), 1.0);
        assert!((fine.area() - 600.0).abs() < 1e-9, "{}", fine.area());
        assert!(fine.longest_edge() <= 1.0 && fine.triangles.len() == 12 * 4usize.pow(4), "{} {}", fine.longest_edge(), fine.triangles.len());
        let mut sides = std::collections::BTreeMap::new();
        for t in &fine.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *sides.entry((a.min(b), a.max(b))).or_insert(0) += 1;
            }
        }
        assert!(sides.values().all(|&n| n == 2), "closed: each edge twice");
        assert_eq!(even(&cube(), 0.0), cube(), "no edge asked: as it came");
    }

    /// Noise of 0.1 moves the points 0.1 (rms) along the faces' normals, the same each time for
    /// the same seed and otherwise for another.
    #[test]
    fn noise_has_the_spread_asked_and_follows_the_seed() {
        let how = Degrade { edge: 0.5, sigma: 0.1, seed: 7, ..Degrade::default() };
        let (clean, noisy) = (even(&cube(), 0.5), degraded(&cube(), &how));
        assert_eq!(noisy, degraded(&cube(), &how), "the same seed: the same scan, bit for bit");
        assert_ne!(noisy, degraded(&cube(), &Degrade { seed: 8, ..how }));
        // Points inside the top face (not on an edge, where the normal leans): moved along Z only.
        let moved: Vec<f64> = clean
            .points
            .iter()
            .zip(&noisy.points)
            .filter(|(p, _)| p[2] == 10.0 && p[0] > 0.0 && p[0] < 10.0 && p[1] > 0.0 && p[1] < 10.0)
            .map(|(p, q)| {
                assert!((p[0] - q[0]).abs() < 1e-12 && (p[1] - q[1]).abs() < 1e-12);
                q[2] - p[2]
            })
            .collect();
        let rms = (moved.iter().map(|d| d * d).sum::<f64>() / moved.len() as f64).sqrt();
        let mean = moved.iter().sum::<f64>() / moved.len() as f64;
        assert!(moved.len() > 300 && (rms - 0.1).abs() < 0.015 && mean.abs() < 0.02, "{} points, rms {rms}, mean {mean}", moved.len());
    }

    /// A fifth of the area missing in three patches takes away about a fifth (less where patches
    /// overlap or run over an edge); a pose and a unit move every point as a rigid body times the
    /// unit; specks add triangles and take none.
    #[test]
    fn missing_patches_a_pose_a_unit_and_specks() {
        let whole = even(&cube(), 0.5);
        let holed = degraded(&cube(), &Degrade { edge: 0.5, missing: 0.2, ..Degrade::default() });
        let gone = 1.0 - holed.area() / whole.area();
        // A triangle goes when its middle is in a patch, so up to half an edge more goes all round.
        assert!(gone > 0.08 && gone <= 0.26, "{gone}");
        let turned = degraded(&cube(), &Degrade { axis: [0.0, 0.0, 2.0], angle: std::f64::consts::FRAC_PI_2, shift: [1.0, 2.0, 3.0], scale: 1.0 / 25.4, ..Degrade::default() });
        // (10, 0, 0) a quarter turn about Z is (0, 10, 0); shifted (1, 12, 3); in inches.
        let p = turned.points[1];
        assert!((p[0] - 1.0 / 25.4).abs() < 1e-12 && (p[1] - 12.0 / 25.4).abs() < 1e-12 && (p[2] - 3.0 / 25.4).abs() < 1e-12, "{p:?}");
        assert!((turned.area() - 600.0 / (25.4 * 25.4)).abs() < 1e-9);
        let specked = degraded(&cube(), &Degrade { edge: 1.0, specks: 0.01, ..Degrade::default() });
        assert_eq!(specked.triangles.len(), whole_count(1.0) + 31, "one in a hundred of 3072");
        let stl = cube().stl();
        assert_eq!(stl.len(), 84 + 12 * 50);
    }

    fn whole_count(edge: f64) -> usize {
        even(&cube(), edge).triangles.len()
    }
}
