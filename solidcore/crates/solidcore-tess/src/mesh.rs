//! The indexed triangle mesh a plan evaluates to, with its edge polylines and how each edge is
//! drawn. Data and presentation only: what a mesh measures (volume, a ray's hits, inside) is the
//! kernel's, never decided from a display mesh.

use serde::{Deserialize, Serialize};
use solidcore_record::Vec3;

use crate::vec::VecExt as _;

/// The normal a flat triangle with corners `a`, `b`, `c` is lit with: its plane's, of unit
/// length, facing where the corners turn counter-clockwise; +Z for one with no area.
#[must_use]
pub fn flat_normal(a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    (b - a)
        .cross(c - a)
        .normalized()
        .unwrap_or(Vec3::new(0.0, 0.0, 1.0))
}

/// An indexed triangle mesh with per-vertex normals, a face id per triangle (the B-rep face it
/// came from) and edge polylines (B-rep edges for display and picking, or mesh outlines).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Mesh {
    /// Vertex positions.
    pub positions: Vec<Vec3>,
    /// Vertex normals, one per position, or none: every triangle lit flat with its own normal
    /// ([`Mesh::is_flat`]).
    pub normals: Vec<Vec3>,
    /// Triangles as vertex index triples (counter-clockwise seen from outside).
    pub triangles: Vec<[u32; 3]>,
    /// B-rep face index per triangle.
    pub tri_face: Vec<u32>,
    /// Edge polylines; mesh outlines have `display_only_edges` set.
    pub edges: Vec<Vec<Vec3>>,
    /// These polylines belong to a triangle mesh, not selectable B-rep edges. Rebuilt with
    /// the display mesh; never part of the saved design's geometry.
    #[serde(skip)]
    pub display_only_edges: bool,
    /// Per edge: a seam between two pieces of the same smooth surface (not drawn).
    #[serde(default)]
    pub seams: Vec<bool>,
    /// Display-only tangent joins. Edge identities, picking and face groups stay intact.
    #[serde(skip)]
    pub smooth_edges: Vec<bool>,
    /// Display-protected boundaries: certified current creases or authored splits.
    /// Overrides coarse seam fits; not a geometric/topological authority.
    #[serde(skip)]
    pub sharp_edges: Vec<bool>,
    /// Per edge: the B-rep faces it bounds (one or two).
    #[serde(default)]
    pub edge_faces: Vec<Vec<u32>>,
}

impl Mesh {
    /// Visibility in shaded edge presentation, independent of selection/topology.
    #[must_use]
    pub fn shaded_edge_visible(&self, index: usize) -> bool {
        self.edge_visible(index) && !self.smooth_edges.get(index).copied().unwrap_or(false)
    }

    /// Visibility in wireframe, including triangle-mesh outlines without B-rep identities.
    #[must_use]
    pub fn edge_visible(&self, index: usize) -> bool {
        !self.seams.get(index).copied().unwrap_or(false)
            || self.sharp_edges.get(index).copied().unwrap_or(false)
    }

    /// Whether edge `index` can be picked: a visible B-rep edge, not a mesh outline.
    #[must_use]
    pub fn edge_pickable(&self, index: usize) -> bool {
        !self.display_only_edges && self.edge_visible(index)
    }

    /// Edges bounding face `f`.
    #[must_use]
    pub fn face_edges(&self, f: u32) -> Vec<usize> {
        self.edge_faces
            .iter()
            .enumerate()
            .filter(|(_, fs)| fs.contains(&f))
            .map(|(i, _)| i)
            .collect()
    }

    /// The corners of triangle `t`, `None` where an index is out of range.
    #[must_use]
    pub fn tri(&self, t: &[u32; 3]) -> Option<[Vec3; 3]> {
        Some([
            *self.positions.get(t[0] as usize)?,
            *self.positions.get(t[1] as usize)?,
            *self.positions.get(t[2] as usize)?,
        ])
    }

    /// Every triangle is lit flat, with its own normal: the mesh has no normals, and its
    /// triangles share their corners (a mesh body's, as a scan's millions of triangles are
    /// kept), so a corner has no normal of its own. [`Mesh::corner_normals`] reads either kind.
    #[must_use]
    pub fn is_flat(&self) -> bool {
        self.normals.is_empty() && !self.triangles.is_empty()
    }

    /// The normals triangle `t`'s corners are lit with: the vertices' own, or in a flat mesh
    /// ([`Mesh::is_flat`]) each the triangle's ([`flat_normal`]). `None` when a corner is out
    /// of range.
    #[must_use]
    pub fn corner_normals(&self, t: &[u32; 3]) -> Option<[Vec3; 3]> {
        if self.normals.is_empty() {
            let [a, b, c] = self.tri(t)?;
            return Some([flat_normal(a, b, c); 3]);
        }
        Some([
            *self.normals.get(t[0] as usize)?,
            *self.normals.get(t[1] as usize)?,
            *self.normals.get(t[2] as usize)?,
        ])
    }

    /// Each triangle's corners with the normals they are lit with ([`Mesh::corner_normals`]),
    /// in order; a triangle with a corner out of range is left out.
    pub fn lit_corners(&self) -> impl Iterator<Item = [(Vec3, Vec3); 3]> + '_ {
        self.triangles.iter().filter_map(|t| {
            let ([a, b, c], [na, nb, nc]) = (self.tri(t)?, self.corner_normals(t)?);
            Some([(a, na), (b, nb), (c, nc)])
        })
    }

    /// This mesh with a normal for every vertex: itself, or a flat one ([`Mesh::is_flat`]) with
    /// each triangle given three corners of its own lit with its normal, in the triangles' order
    /// (a triangle with a corner out of range left out). For a reader that takes normals by
    /// vertex; it costs a large mesh several times its memory, so a viewport reads
    /// [`Mesh::corner_normals`] instead.
    #[must_use]
    pub fn with_normals(&self) -> std::borrow::Cow<'_, Mesh> {
        if !self.is_flat() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut out = Mesh {
            edges: self.edges.clone(),
            display_only_edges: self.display_only_edges,
            ..Mesh::default()
        };
        out.seams.clone_from(&self.seams);
        out.smooth_edges.clone_from(&self.smooth_edges);
        out.sharp_edges.clone_from(&self.sharp_edges);
        out.edge_faces.clone_from(&self.edge_faces);
        out.positions.reserve_exact(self.triangles.len() * 3);
        out.normals.reserve_exact(self.triangles.len() * 3);
        out.triangles.reserve_exact(self.triangles.len());
        for (i, t) in self.triangles.iter().enumerate() {
            let Some([a, b, c]) = self.tri(t) else {
                continue;
            };
            let base = u32::try_from(out.positions.len()).unwrap_or(u32::MAX);
            out.positions.extend([a, b, c]);
            out.normals.extend([flat_normal(a, b, c); 3]);
            out.triangles
                .push([base, base.saturating_add(1), base.saturating_add(2)]);
            out.tri_face.extend(self.tri_face.get(i));
        }
        std::borrow::Cow::Owned(out)
    }
}
