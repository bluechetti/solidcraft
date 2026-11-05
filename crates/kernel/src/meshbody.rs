//! Mesh bodies: triangles without a B-rep (imported 3MF/STL). They render, measure, move and
//! export; B-rep features refuse them.

use solidcraft_geom::prelude::*;
use std::collections::HashMap;

use solidcore_record::Vec3;
use solidcraft_geom::Mesh;

use crate::body::Body;
use crate::{KernelError, Result};

/// Most triangles in one mesh body, and most vertices a mesh file may give one: far above any
/// scan's (a 28-million-triangle statue takes about 10 GB to work with) and well within the
/// `u32` indices of its triangles. What keeps the memory a file can ask for in check is the
/// largest file read (`--max-file-size`), which the readers never allocate past.
pub const MAX_TRIANGLES: usize = 250_000_000;
pub const MAX_VERTICES: usize = 250_000_000;

/// Indexed triangles with shared vertices, wound counter-clockwise seen from outside.
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

impl TriMesh {
    /// The mesh a body keeps ([`Body::display_mesh`]): these triangles in order, sharing their
    /// corners, each lit flat ([`Mesh::is_flat`]: no normals), with the body's outlines
    /// ([`outlines`]). One copy serves the kernel, the viewport and picking: a scan's
    /// triangles are not held twice.
    pub(crate) fn into_kept(self) -> Mesh {
        let edges = outlines(&self.positions, &self.triangles);
        Mesh {
            tri_face: vec![0; self.triangles.len()],
            positions: self.positions,
            triangles: self.triangles,
            edges,
            display_only_edges: true,
            ..Mesh::default()
        }
    }

    /// Signed volume ×6.
    fn volume6(&self) -> f64 {
        self.triangles
            .iter()
            .filter_map(|t| Some((self.positions.get(t[0] as usize)?, self.positions.get(t[1] as usize)?, self.positions.get(t[2] as usize)?)))
            .map(|(a, b, c)| a.dot(b.cross(*c)))
            .sum()
    }
}

/// A mesh body from triangles (`positions` in mm). Coincident vertices are merged, degenerate
/// triangles dropped, and a closed mesh with inward winding is turned outward.
pub fn mesh_body(positions: &[Vec3], triangles: &[[u32; 3]]) -> Result<Body> {
    body_from(positions.iter().copied(), positions.len(), triangles)
}

/// [`mesh_body`] from positions held flat, x, y, z for each (as a design keeps a mesh import),
/// without a copy of them or of the triangles.
pub fn mesh_body_flat(positions: &[f64], triangles: &[[u32; 3]]) -> Result<Body> {
    let (points, rest) = positions.as_chunks::<3>();
    if !rest.is_empty() {
        return Err(KernelError::Invalid("mesh positions are not whole points".into()));
    }
    body_from(points.iter().map(|c| Vec3::new(c[0], c[1], c[2])), points.len(), triangles)
}

/// What [`mesh_body`] refuses, checked without building the body: too many triangles, a
/// coordinate that is not finite or is huge, a corner that names no vertex, or no triangle with
/// three different corners. An import checks a file with it; the body is built once, when the
/// design is worked out.
pub fn check_mesh(positions: &[Vec3], triangles: &[[u32; 3]]) -> Result<()> {
    check_counts_and_points(positions.iter().copied(), triangles)?;
    let mut any = false;
    for t in triangles {
        let (Some(a), Some(b), Some(c)) = (positions.get(t[0] as usize), positions.get(t[1] as usize), positions.get(t[2] as usize)) else {
            return Err(missing_vertex());
        };
        // Different corners after the exact merge: different coordinates, bit for bit.
        let (a, b, c) = (bits(*a), bits(*b), bits(*c));
        any |= a != b && b != c && c != a;
    }
    if !any {
        return Err(no_triangles());
    }
    Ok(())
}

/// Exactly coincident vertices merged, first come first: the positions kept and every triangle's
/// corners in them (a degenerate triangle too). A design keeps an imported mesh welded so (an STL
/// file gives every triangle its own three corners); [`mesh_body`] merges the same way, so the
/// body is the same either way.
pub fn weld_exact(positions: &[Vec3], triangles: &[[u32; 3]]) -> Result<(Vec<Vec3>, Vec<[u32; 3]>)> {
    weld(positions.iter().copied(), positions.len(), triangles)
}

fn body_from(positions: impl Iterator<Item = Vec3> + Clone, count: usize, triangles: &[[u32; 3]]) -> Result<Body> {
    let _span = solidcraft_profile::span("kernel", "mesh_body");
    check_counts_and_points(positions.clone(), triangles)?;
    let (positions, mut triangles) = weld(positions, count, triangles)?;
    triangles.retain(|t| t[0] != t[1] && t[1] != t[2] && t[2] != t[0]);
    if triangles.is_empty() {
        return Err(no_triangles());
    }
    let mut out = TriMesh { positions, triangles };
    unify_winding(&mut out);
    if out.volume6() < 0.0 && is_closed(&out.triangles) {
        for t in &mut out.triangles {
            t.swap(1, 2);
        }
    }
    Ok(Body::of_mesh(out))
}

fn check_counts_and_points(mut positions: impl Iterator<Item = Vec3>, triangles: &[[u32; 3]]) -> Result<()> {
    if triangles.len() > MAX_TRIANGLES {
        return Err(KernelError::Invalid(format!("mesh with {} triangles (limit {MAX_TRIANGLES})", triangles.len())));
    }
    if positions.any(|p| !(p.is_finite() && p.x.abs() < 1e9 && p.y.abs() < 1e9 && p.z.abs() < 1e9)) {
        return Err(KernelError::Invalid("mesh with non-finite or huge coordinates".into()));
    }
    Ok(())
}

fn missing_vertex() -> KernelError {
    KernelError::Invalid("mesh triangle refers to a missing vertex".into())
}

fn no_triangles() -> KernelError {
    KernelError::Invalid("mesh has no triangles".into())
}

/// A vertex's coordinates bit for bit, -0 as 0: the key exactly coincident vertices share.
fn bits(p: Vec3) -> [u64; 3] {
    [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()]
}

/// [`weld_exact`] of `count` positions.
fn weld(positions: impl Iterator<Item = Vec3>, count: usize, triangles: &[[u32; 3]]) -> Result<(Vec<Vec3>, Vec<[u32; 3]>)> {
    let mut map: HashMap<[u64; 3], u32> = HashMap::new();
    let mut remap: Vec<u32> = Vec::with_capacity(count);
    let mut kept = Vec::new();
    for p in positions {
        let next = u32::try_from(kept.len()).map_err(|_| KernelError::Invalid("too many vertices".into()))?;
        let i = *map.entry(bits(p)).or_insert_with(|| {
            kept.push(p);
            next
        });
        remap.push(i);
    }
    drop(map);
    let corner = |i: u32| remap.get(i as usize).copied().ok_or_else(missing_vertex);
    let triangles = triangles.iter().map(|t| Ok([corner(t[0])?, corner(t[1])?, corner(t[2])?])).collect::<Result<Vec<_>>>()?;
    Ok((kept, triangles))
}

/// The top bit of a side's place in [`sorted_sides`]: set when the side runs from its lower end.
/// The place itself, `3 * triangle + k`, stays below it (`3 * MAX_TRIANGLES < 2^31`).
const FROM_LOWER: u32 = 1 << 31;

/// Every side of every triangle by its ends, lower first, with where it is (`3 * triangle + k`,
/// side `k` running from corner `k` to the next, with [`FROM_LOWER`] when it runs from the lower
/// end); sorted, so the sides that triangles share lie next to one another. A sort of 12 bytes a
/// side rather than a map of the triangles at each side, whose small lists took gigabytes for a
/// 28-million-triangle scan.
fn sorted_sides(triangles: &[[u32; 3]]) -> Vec<(u32, u32, u32)> {
    let mut sides = Vec::with_capacity(triangles.len().saturating_mul(3));
    for (i, t) in triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if let Ok(at) = u32::try_from(3 * i + k)
                && at < FROM_LOWER
            {
                sides.push((a.min(b), a.max(b), if a < b { at | FROM_LOWER } else { at }));
            }
        }
    }
    crate::par::sort_unstable(&mut sides);
    sides
}

/// Wind every connected piece one way: a triangle that runs a side in the same direction as
/// its neighbour is turned over (files with stray inverted facets). Sides shared by more than
/// two triangles are left alone.
fn unify_winding(m: &mut TriMesh) {
    let n = m.triangles.len();
    // The side across each side: the other triangle's, where exactly two triangles share it.
    let mut across = vec![u32::MAX; n.saturating_mul(3)];
    for group in sorted_sides(&m.triangles).chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)) {
        if let [(_, _, s), (_, _, r)] = group {
            let (s, r) = (s & !FROM_LOWER, r & !FROM_LOWER);
            if let Some(x) = across.get_mut(s as usize) {
                *x = r;
            }
            if let Some(x) = across.get_mut(r as usize) {
                *x = s;
            }
        }
    }
    // Each piece walked from its first triangle, which keeps its winding: a neighbour that runs
    // the side they share the way the triangle it was reached from runs it is turned over. The
    // turns are kept aside and made at the end, so `across` still names the file's sides.
    let side = |t: &[u32; 3], k: usize| (t[k], t[(k + 1) % 3]);
    let mut turned = vec![false; n];
    let mut done = vec![false; n];
    for start in 0..n {
        if done.get(start).copied().unwrap_or(true) {
            continue;
        }
        if let Some(d) = done.get_mut(start) {
            *d = true;
        }
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            let (Some(t), Some(&flip)) = (m.triangles.get(i).copied(), turned.get(i)) else { continue };
            // The sides in the order the triangle runs them as it is now.
            let order = if flip { [2, 1, 0] } else { [0, 1, 2] };
            for k in order {
                let Some(&s) = across.get(3 * i + k) else { continue };
                let (j, kj) = (s as usize / 3, s as usize % 3);
                if s == u32::MAX || done.get(j).copied().unwrap_or(true) {
                    continue;
                }
                let Some(u) = m.triangles.get(j) else { continue };
                if let Some(d) = done.get_mut(j) {
                    *d = true;
                }
                if let Some(x) = turned.get_mut(j) {
                    *x = flip ^ (side(&t, k) == side(u, kj));
                }
                stack.push(j);
            }
        }
    }
    for (t, turn) in m.triangles.iter_mut().zip(turned) {
        if turn {
            t.swap(1, 2);
        }
    }
}

/// Display lines for an imported triangle mesh: open boundaries, non-manifold sides and
/// creases of at least 30 degrees, unless the creases are a scan's noise ([`noise_creases`]).
/// Worked out once, when the body is made ([`TriMesh::into_kept`]).
pub(crate) fn outlines(positions: &[Vec3], triangles: &[[u32; 3]]) -> Vec<Vec<Vec3>> {
    let normal = |index: usize| {
        let t = triangles.get(index)?;
        let (a, b, c) = (positions.get(t[0] as usize)?, positions.get(t[1] as usize)?, positions.get(t[2] as usize)?);
        (*b - *a).cross(*c - *a).normalized()
    };
    // One compact record per side, sorted for deterministic output, instead of a map of
    // separately allocated adjacency lists. Mesh imports already cap the triangle count.
    let mut sides = Vec::with_capacity(triangles.len().saturating_mul(3));
    for (i, t) in triangles.iter().enumerate() {
        let Ok(triangle) = u32::try_from(i) else { continue };
        if normal(i).is_some() {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                sides.push((a.min(b), a.max(b), triangle));
            }
        }
    }
    crate::par::sort_unstable(&mut sides);
    let cos = 30f64.to_radians().cos();
    // Each outline side, in the sorted order, and whether it is a crease.
    let mut outline = Vec::new();
    for side in sides.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
        let Some(&(a, b, _)) = side.first() else { continue };
        let (visible, crease) = match side {
            [(_, _, i), (_, _, j)] => (normal(*i as usize).zip(normal(*j as usize)).is_some_and(|(n, m)| n.dot(m) <= cos), true),
            _ => (true, false),
        };
        if visible && positions.get(a as usize).is_some() && positions.get(b as usize).is_some() {
            outline.push((a, b, crease));
        }
    }
    let noise = noise_creases(positions.len(), &outline);
    let edges: Vec<(u32, u32)> = outline.iter().filter(|s| !(noise && s.2)).map(|&(a, b, _)| (a, b)).collect();
    straight_runs(positions, &edges)
}

/// The least creases a mesh has before they can be taken for noise: a small mesh keeps them all.
const NOISE_CREASES: usize = 100;

/// Whether a mesh's creases are a scan's noise, not its edges: more than a tenth of them end
/// where no other outline meets. A part's creases run on into one another, round its faces, and
/// end at its corners or open edges (2% at most end loose in the CAD meshes measured, none in a
/// box or a faceted cylinder); a scan's noise is short creases scattered over it (16 to 58% end
/// loose in the scans measured), drawn as a speckle that hides its shape. `outline` is each
/// outline side with whether it is a crease; `vertices` is the mesh's count.
fn noise_creases(vertices: usize, outline: &[(u32, u32, bool)]) -> bool {
    let creases = outline.iter().filter(|s| s.2).count();
    if creases < NOISE_CREASES {
        return false;
    }
    let mut meets = vec![0u8; vertices];
    for &(a, b, _) in outline {
        for v in [a, b] {
            if let Some(m) = meets.get_mut(v as usize) {
                *m = m.saturating_add(1);
            }
        }
    }
    let alone = |v: u32| meets.get(v as usize).is_some_and(|m| *m == 1);
    let loose = outline.iter().filter(|&&(a, b, crease)| crease && (alone(a) || alone(b))).count();
    loose * 10 > creases
}

/// The outline sides `sides` (vertex index pairs) as polylines: sides that run on straight
/// through a vertex no other outline side meets are one line. A mesh whose straight edges are
/// in pieces (a box exported at a grid's spacing) then has one outline per edge, as its CAD
/// model did, and a click on it measures the whole edge.
fn straight_runs(positions: &[Vec3], sides: &[(u32, u32)]) -> Vec<Vec<Vec3>> {
    let mut at: std::collections::BTreeMap<u32, Vec<usize>> = std::collections::BTreeMap::new();
    for (i, &(a, b)) in sides.iter().enumerate() {
        at.entry(a).or_default().push(i);
        at.entry(b).or_default().push(i);
    }
    let point = |v: u32| positions.get(v as usize).copied().unwrap_or(Vec3::ZERO);
    let other = |i: usize, v: u32| sides.get(i).map_or(v, |&(a, b)| if a == v { b } else { a });
    // The side that continues `i` straight on through `v`, when `v` joins exactly two.
    let next = |i: usize, v: u32| -> Option<usize> {
        let [p, q] = at.get(&v)?.as_slice() else { return None };
        let j = if *p == i { *q } else { *p };
        let (into, out) = ((point(v) - point(other(i, v))).normalized()?, (point(other(j, v)) - point(v)).normalized()?);
        (into.dot(out) > 1.0 - 1e-9).then_some(j)
    };
    let mut used = vec![false; sides.len()];
    // Marks side `i` used; false when it already was.
    let mut take = |i: usize| used.get_mut(i).is_some_and(|u| !std::mem::replace(u, true));
    let mut runs = Vec::new();
    for (i, &(a, b)) in sides.iter().enumerate() {
        if !take(i) {
            continue;
        }
        let mut run = std::collections::VecDeque::from([a, b]);
        for (mut side, mut end, front) in [(i, b, false), (i, a, true)] {
            while let Some(j) = next(side, end).filter(|j| take(*j)) {
                end = other(j, end);
                side = j;
                if front {
                    run.push_front(end);
                } else {
                    run.push_back(end);
                }
            }
        }
        runs.push(run.into_iter().map(point).collect());
    }
    runs
}

/// The mesh a body keeps as it is shown: itself when every vertex is a triangle's corner, else a
/// copy without the others ([`Body::display_mesh`]).
fn shown(kept: &std::sync::Arc<Mesh>) -> std::sync::Arc<Mesh> {
    let n = kept.positions.len();
    let mut used = vec![false; n];
    let mut valid = true;
    for &i in kept.triangles.iter().flatten() {
        match used.get_mut(i as usize) {
            Some(u) => *u = true,
            None => valid = false,
        }
    }
    if valid && used.iter().all(|u| *u) {
        return kept.clone();
    }
    let mut at = vec![u32::MAX; n];
    let mut positions = Vec::new();
    for ((p, u), a) in kept.positions.iter().zip(&used).zip(&mut at) {
        if *u {
            *a = u32::try_from(positions.len()).unwrap_or(u32::MAX);
            positions.push(*p);
        }
    }
    let corner = |i: u32| at.get(i as usize).copied().filter(|a| *a != u32::MAX);
    let triangles: Vec<[u32; 3]> = kept.triangles.iter().filter_map(|t| Some([corner(t[0])?, corner(t[1])?, corner(t[2])?])).collect();
    std::sync::Arc::new(Mesh {
        tri_face: vec![0; triangles.len()],
        positions,
        triangles,
        edges: kept.edges.clone(),
        display_only_edges: kept.display_only_edges,
        ..Mesh::default()
    })
}

/// Distinct undirected triangle sides.
pub(crate) fn edge_count(triangles: &[[u32; 3]]) -> usize {
    sorted_sides(triangles).chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)).count()
}

/// Every triangle side is shared by exactly two triangles in opposite directions.
pub(crate) fn is_closed(triangles: &[[u32; 3]]) -> bool {
    sorted_sides(triangles).chunk_by(|x, y| (x.0, x.1) == (y.0, y.1)).all(|g| matches!(g, [(_, _, p), (_, _, q)] if (p ^ q) & FROM_LOWER != 0))
}

impl Body {
    /// Shared-vertex triangles of a mesh body.
    pub fn triangle_mesh(&self) -> Option<(&[Vec3], &[[u32; 3]])> {
        self.mesh.as_ref().map(|m| (m.positions.as_slice(), m.triangles.as_slice()))
    }
    /// Does a mesh body enclose a volume (every side shared by two triangles)?
    pub fn is_closed_mesh(&self) -> bool {
        self.mesh.as_ref().is_some_and(|m| is_closed(&m.triangles))
    }

    /// A mesh body's triangles as the viewport draws and picks them: the mesh the body keeps
    /// ([`TriMesh::into_kept`]), shared, not copied. A vertex no triangle uses (one only a
    /// dropped degenerate triangle had) stays in the body, so its vertices match the file's;
    /// when there is one, the mesh shown is a copy without it, so its box is its triangles'.
    pub fn display_mesh(&self) -> Option<std::sync::Arc<Mesh>> {
        self.mesh.as_ref().map(shown)
    }

    /// A mesh body moved by a column-major affine matrix (reflections keep outward winding).
    pub(crate) fn mesh_transformed(&self, m: &[[f64; 4]; 4]) -> Option<Body> {
        let tm = self.mesh.as_ref()?;
        let f = |p: Vec3| {
            Vec3::new(
                m[0][0] * p.x + m[1][0] * p.y + m[2][0] * p.z + m[3][0],
                m[0][1] * p.x + m[1][1] * p.y + m[2][1] * p.z + m[3][1],
                m[0][2] * p.x + m[1][2] * p.y + m[2][2] * p.z + m[3][2],
            )
        };
        let det = m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2]) - m[1][0] * (m[0][1] * m[2][2] - m[2][1] * m[0][2])
            + m[2][0] * (m[0][1] * m[1][2] - m[1][1] * m[0][2]);
        let mut out = TriMesh { positions: tm.positions.iter().map(|p| f(*p)).collect(), triangles: tm.triangles.clone() };
        if det < 0.0 {
            for t in &mut out.triangles {
                t.swap(1, 2);
            }
        }
        let mut placed = Body::of_mesh(out);
        placed.color = self.color;
        placed.paint = self.paint.clone();
        placed.splits = self.splits.clone();
        placed.regularization_protected = self.regularization_protected;
        Some(placed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walk [`unify_winding`] replaced (a map from each side to its triangles), kept to check
    /// that the sorted sides turn the same triangles.
    fn unify_winding_by_map(m: &mut TriMesh) {
        let mut sides: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
        for (i, t) in m.triangles.iter().enumerate() {
            for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                sides.entry((a.min(b), a.max(b))).or_default().push(i);
            }
        }
        let runs = |t: &[u32; 3], a: u32, b: u32| [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])].contains(&(a, b));
        let mut done = vec![false; m.triangles.len()];
        for start in 0..m.triangles.len() {
            if done[start] {
                continue;
            }
            done[start] = true;
            let mut stack = vec![start];
            while let Some(i) = stack.pop() {
                let t = m.triangles[i];
                for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
                    let list = &sides[&(a.min(b), a.max(b))];
                    if list.len() != 2 {
                        continue;
                    }
                    for &j in list {
                        if j == i || done[j] {
                            continue;
                        }
                        done[j] = true;
                        if runs(&m.triangles[j], a, b) {
                            m.triangles[j].swap(1, 2);
                        }
                        stack.push(j);
                    }
                }
            }
        }
    }

    /// A band of `n` squares round and `rows` along, two triangles each; closed round into a tube,
    /// or with a half twist into a Möbius strip. Triangles picked by `flip` are turned over.
    fn band(n: u32, rows: u32, twist: bool, flip: impl Fn(usize) -> bool) -> TriMesh {
        let positions = (0..=rows).flat_map(|j| (0..n).map(move |i| Vec3::new(f64::from(i), f64::from(j), f64::from(i * j % 7)))).collect();
        let at = |i: u32, j: u32| if i == n { if twist { (rows - j) * n } else { j * n } } else { j * n + i };
        let mut triangles: Vec<[u32; 3]> = (0..rows)
            .flat_map(|j| (0..n).flat_map(move |i| [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]]))
            .collect();
        for (k, t) in triangles.iter_mut().enumerate() {
            if flip(k) {
                t.swap(1, 2);
            }
        }
        TriMesh { positions, triangles }
    }

    #[test]
    fn sorted_sides_turn_the_triangles_the_map_did() {
        let stray = |k: usize| k % 7 == 3 || k.is_multiple_of(11);
        let mut fin = band(12, 6, false, stray);
        // A fin on one side: shared by three triangles, it is left alone.
        let extra = u32::try_from(fin.positions.len()).unwrap();
        fin.positions.push(Vec3::new(0.5, 0.5, 9.0));
        let [a, b, _] = fin.triangles[0];
        fin.triangles.push([a, b, extra]);
        for mesh in [band(12, 6, false, stray), band(9, 4, true, stray), band(9, 5, true, |_| false), fin] {
            let (mut sorted, mut mapped) = (mesh.clone(), mesh);
            unify_winding(&mut sorted);
            unify_winding_by_map(&mut mapped);
            assert_eq!(sorted.triangles, mapped.triangles);
        }
        // A tube is open at its ends, however it is wound; its sides are counted once each: 12
        // round each of its 7 rings, and 12 along and 12 across each of its 6 rows.
        let mut tube = band(12, 6, false, stray);
        unify_winding(&mut tube);
        assert!(!is_closed(&tube.triangles));
        assert_eq!(edge_count(&tube.triangles), 12 * 7 + 2 * 12 * 6);
    }

    /// `check_mesh` refuses what building the body refuses, with the same words, without
    /// building it; positions held flat, or welded first, make the same body.
    #[test]
    fn a_mesh_is_checked_as_it_is_built_and_built_the_same_from_any_form() {
        let square = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let twin = [Vec3::ZERO, Vec3::new(-0.0, 0.0, 0.0), Vec3::X];
        let cases: [(&[Vec3], &[[u32; 3]]); 6] = [
            (&square, &[[0, 1, 2], [0, 2, 3]]),
            (&square, &[[0, 1, 9]]),
            (&[Vec3::new(f64::NAN, 0.0, 0.0), Vec3::X, Vec3::Y], &[[0, 1, 2]]),
            (&[Vec3::new(2e9, 0.0, 0.0), Vec3::X, Vec3::Y], &[[0, 1, 2]]),
            (&square, &[[0, 0, 1]]),
            // -0 and 0 are one vertex: the triangle has two corners alike.
            (&twin, &[[0, 1, 2]]),
        ];
        for (positions, triangles) in cases {
            let built = mesh_body(positions, triangles).map(|_| ()).map_err(|e| e.to_string());
            let checked = check_mesh(positions, triangles).map_err(|e| e.to_string());
            assert_eq!(checked, built, "{positions:?} {triangles:?}");
        }
        // Every triangle with its own corners, as an STL gives them.
        let corners: Vec<Vec3> = [[0, 1, 2], [0, 2, 3]].iter().flat_map(|t: &[usize; 3]| t.map(|i| square[i])).collect();
        let separate = [[0, 1, 2], [3, 4, 5]];
        let (welded, triangles) = weld_exact(&corners, &separate).unwrap();
        assert_eq!((welded.len(), triangles.as_slice()), (4, [[0, 1, 2], [0, 2, 3]].as_slice()));
        let flat: Vec<f64> = corners.iter().flat_map(|p| [p.x, p.y, p.z]).collect();
        let body = |b: Body| b.triangle_mesh().map(|(p, t)| (p.to_vec(), t.to_vec()));
        let want = body(mesh_body(&corners, &separate).unwrap());
        assert_eq!(body(mesh_body(&welded, &triangles).unwrap()), want);
        assert_eq!(body(mesh_body_flat(&flat, &separate).unwrap()), want);
        assert!(mesh_body_flat(&flat[1..], &separate).is_err(), "not whole points");
    }

    #[test]
    fn mesh_outlines_keep_boundaries_and_creases_without_smooth_facet_lines() {
        // Flat (0 degrees), the sides from Y through the origin to -Y run straight on: one line.
        for (degrees, expected) in [(0.0f64, 3), (15.0, 4), (30.0, 5), (45.0, 5), (90.0, 5)] {
            let angle = degrees.to_radians();
            let p = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::new(0., -angle.cos(), angle.sin())];
            let triangles = [[0, 1, 2], [1, 0, 3]];
            let edges = outlines(&p, &triangles);
            assert_eq!(edges.len(), expected, "hinge angle {degrees}");
            assert_eq!(edges.iter().any(|e| e == &[Vec3::ZERO, Vec3::X]), degrees >= 30.);
            assert_eq!(edges, outlines(&p, &triangles), "deterministic order");
        }
        // A third triangle makes the common side non-manifold: keep it once, even if coplanar.
        let p = [Vec3::ZERO, Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Y * 2.];
        let edges = outlines(&p, &[[0, 1, 2], [1, 0, 3], [0, 1, 4]]);
        assert_eq!(edges.len(), 7);
        assert_eq!(edges.iter().filter(|e| **e == [Vec3::ZERO, Vec3::X]).count(), 1);
        // The rings of an open faceted cylinder stay visible, but its smooth wall has no
        // diagonals or longitudinal facet lines.
        let n = 36u32;
        let mut p = Vec::new();
        for z in [0., 10.] {
            p.extend((0..n).map(|i| {
                let a = f64::from(i) * std::f64::consts::TAU / f64::from(n);
                Vec3::new(a.cos(), a.sin(), z)
            }));
        }
        let triangles: Vec<_> = (0..n)
            .flat_map(|i| {
                let j = (i + 1) % n;
                [[i, j, j + n], [i, j + n, i + n]]
            })
            .collect();
        let edges = outlines(&p, &triangles);
        assert_eq!(edges.len(), 2 * n as usize);
        assert!(edges.iter().all(|e| e[0].z == e[1].z));
        // A straight edge in pieces (a node at its middle, where the facets of the face meet)
        // is one outline; its corners stay corners.
        let p = [Vec3::ZERO, Vec3::X * 2., Vec3::new(2., 1., 0.), Vec3::Y, Vec3::X];
        let edges = outlines(&p, &[[0, 4, 3], [4, 1, 2], [4, 2, 3]]);
        assert_eq!(edges.len(), 4, "{edges:?}");
        assert!(edges.iter().any(|e| e == &[Vec3::ZERO, Vec3::X, Vec3::X * 2.] || e == &[Vec3::X * 2., Vec3::X, Vec3::ZERO]), "{edges:?}");
        // Bad indices and zero-area facets do not invent outlines or panic.
        assert!(outlines(&[Vec3::ZERO, Vec3::X, Vec3::X * 2.], &[[0, 1, 99], [0, 1, 2]]).is_empty());
    }

    /// A grid of `n` × `n` unit squares, its height at each node `z(i, j)`.
    fn grid(n: u32, z: impl Fn(u32, u32) -> f64) -> (Vec<Vec3>, Vec<[u32; 3]>) {
        let positions = (0..=n).flat_map(|j| (0..=n).map(move |i| (i, j))).map(|(i, j)| Vec3::new(f64::from(i), f64::from(j), z(i, j))).collect();
        let at = |i: u32, j: u32| j * (n + 1) + i;
        let triangles = (0..n)
            .flat_map(|j| (0..n).flat_map(move |i| [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]]))
            .collect();
        (positions, triangles)
    }

    /// A scan's noise, short creases scattered over it, draws no lines (a speckle that hides its
    /// shape): only its open boundary. A part's creases, folds that run right across it, all stay.
    #[test]
    fn a_scans_noise_creases_are_not_outlines() {
        let n = 40;
        // Heights within ±0.25 of a unit grid: 418 short creases, three in four ending loose.
        let bumpy = |i: u32, j: u32| 0.5 * (f64::from((i.wrapping_mul(73_856_093) ^ j.wrapping_mul(19_349_663)) % 1000) / 1000.0 - 0.5);
        let (p, t) = grid(n, bumpy);
        let edges = outlines(&p, &t);
        let border = |q: &Vec3| q.x == 0.0 || q.y == 0.0 || q.x == f64::from(n) || q.y == f64::from(n);
        assert!(
            !edges.is_empty() && edges.iter().flatten().all(border),
            "only the open boundary: {:?}",
            edges.iter().find(|e| !e.iter().all(border))
        );
        // Folded up and down every 8 squares: four folds right across, 160 crease sides.
        let zigzag = |i: u32, _: u32| f64::from(if (i / 8).is_multiple_of(2) { i % 8 } else { 8 - i % 8 });
        let (p, t) = grid(n, zigzag);
        let edges = outlines(&p, &t);
        for x in [8.0, 16.0, 24.0, 32.0] {
            let across =
                |e: &Vec<Vec3>| e.iter().all(|q| q.x == x) && e.first().zip(e.last()).is_some_and(|(a, b)| (a.y - b.y).abs() == f64::from(n));
            assert!(edges.iter().any(across), "the fold at x = {x}");
        }
    }

    /// A mesh body's display mesh keeps its triangles' shared corners and no normals, each
    /// triangle lit flat: a scan's mesh is not six times the size of its vertices. Given a
    /// corner of its own per triangle, it is the mesh it was when every corner was split, and
    /// a vertex only a dropped triangle had is not in it.
    #[test]
    fn a_mesh_bodys_display_mesh_shares_its_corners_and_lights_each_triangle_flat() {
        // A tetrahedron, as an STL gives it (every triangle its own corners), with a needle
        // triangle whose third corner no other triangle has.
        let v = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z, Vec3::new(5.0, 5.0, 5.0)];
        let faces = [[0usize, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2], [0, 0, 4]];
        let corners: Vec<Vec3> = faces.iter().flat_map(|t| t.map(|i| v[i])).collect();
        let separate: Vec<[u32; 3]> = (0..faces.len() as u32).map(|k| [3 * k, 3 * k + 1, 3 * k + 2]).collect();
        let body = mesh_body(&corners, &separate).unwrap();
        let mesh = body.mesh(&crate::MeshTolerance::chord(0.1)).unwrap();
        assert!(mesh.is_flat());
        assert_eq!((mesh.positions.len(), mesh.triangles.len(), mesh.tri_face.len()), (4, 4, 4), "the needle's far corner is gone");
        assert!(mesh.bounds().max.x <= 1.0);
        let (positions, triangles) = body.triangle_mesh().unwrap();
        let split = mesh.with_normals();
        assert_eq!((split.positions.len(), split.normals.len(), split.triangles.len()), (12, 12, 4));
        for (k, t) in triangles.iter().enumerate() {
            let [a, b, c] = t.map(|i| positions[i as usize]);
            let n = (b - a).cross(c - a).normalized().unwrap();
            assert_eq!(split.tri(&split.triangles[k]).unwrap(), [a, b, c]);
            assert_eq!(mesh.tri(&mesh.triangles[k]).unwrap(), [a, b, c]);
            assert_eq!(split.corner_normals(&split.triangles[k]).unwrap(), [n; 3]);
            assert_eq!(mesh.corner_normals(&mesh.triangles[k]).unwrap(), [n; 3]);
            assert!(n.dot((a + b + c) / 3.0 - Vec3::new(0.25, 0.25, 0.25)) > 0.0, "lit from outside");
        }
        assert!((mesh.measure().volume - 1.0 / 6.0).abs() < 1e-12);
        assert!(body.classify_point(Vec3::new(0.1, 0.1, 0.1)).is_ok_and(|c| c == crate::PointClass::Inside));
        // The body keeps the needle's far corner (its vertices are the file's, welded), so the
        // mesh shown is a copy without it, with the body's outlines: the tetrahedron's six sides.
        assert_eq!(positions.len(), 5);
        let shown = body.display_mesh().unwrap();
        assert_eq!((shown.positions.len(), shown.triangles.len(), shown.edges.len()), (4, 4, 6));
        assert!(shown.is_flat() && shown.display_only_edges && mesh.edges.is_empty());
        // Without one, what is shown is the very mesh the body keeps, not a copy.
        let tetra = mesh_body(&corners[..12], &separate[..4]).unwrap();
        let (a, b) = (tetra.display_mesh().unwrap(), tetra.display_mesh().unwrap());
        assert!(std::sync::Arc::ptr_eq(&a, &b) && std::ptr::eq(a.positions.as_ptr(), tetra.triangle_mesh().unwrap().0.as_ptr()));
        assert_eq!((a.positions.len(), a.edges.len()), (4, 6));
        assert_eq!(crate::measure(&tetra).unwrap().edges, 6);
    }
}
