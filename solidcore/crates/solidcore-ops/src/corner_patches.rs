//! The patches round a corner where other than four faces of a bicubic patch network meet,
//! curvature continuous with one another and with the faces round them
//! ([`solidcore::nurbs::g2_corner`]). Solidcore works out, once for each number of faces, how
//! every point of those patches blends the corner's two-ring of network points; ops hands an
//! application that blend, and the patches of a two-ring given as plain numbers.
//!
//! An application that fits a network to data needs the blend itself, not only the patches: the
//! network's own points are its unknowns, and each place on a corner's patches is a fixed blend
//! of them ([`place_blend`]).

use solidcore::Vec3;

pub use solidcore::nurbs::g2_corner::{CornerBasis, DEGREE, MAX_FACES, MIN_FACES, POINTS, SECTOR};

use crate::error::status;
use crate::Result;

/// The blends for a corner of `faces` faces ([`CornerBasis::for_faces`]). The work grows with
/// the cube of `faces`: an application that meets many corners keeps one basis for each number.
///
/// # Errors
/// [`crate::Error::Refused`] for fewer than [`MIN_FACES`] or more than [`MAX_FACES`] faces.
pub fn corner_basis(faces: usize) -> Result<CornerBasis> {
    CornerBasis::for_faces(faces).map_err(status)
}

/// The number of a two-ring point of a corner of `faces` faces: the corner itself for `jk` =
/// (0, 0), else sector `sector`'s point `jk` of [`SECTOR`]
/// ([`solidcore::nurbs::g2_corner::ring_index`]).
///
/// # Errors
/// [`crate::Error::Refused`] for no faces, or a `jk` that is no point of the two-ring.
pub fn ring_index(faces: usize, sector: i64, jk: (u8, u8)) -> Result<usize> {
    solidcore::nurbs::g2_corner::ring_index(faces, sector, jk).map_err(status)
}

/// The blend of patch `patch`'s point `point`: a weight for each two-ring point
/// ([`CornerBasis::blend`]).
///
/// # Errors
/// [`crate::Error::Refused`] for a patch or a point the corner does not have.
pub fn point_blend(basis: &CornerBasis, patch: usize, point: usize) -> Result<&[f64]> {
    basis.blend(patch, point).map_err(status)
}

/// The place (u, v) on patch `patch` as a blend of the two-ring: a weight for each two-ring
/// point ([`CornerBasis::place_blend`]).
///
/// # Errors
/// [`crate::Error::Refused`] for a patch the corner does not have.
pub fn place_blend(basis: &CornerBasis, patch: usize, u: f64, v: f64) -> Result<Vec<f64>> {
    basis.place_blend(patch, u, v).map_err(status)
}

/// The corner's patches for the two-ring `ring`: [`POINTS`] points each, patch after patch,
/// point (a, b) of a patch the `a + 8 b`-th ([`CornerBasis::patches`]).
///
/// # Errors
/// [`crate::Error::Refused`] when `ring` does not hold one point and six for each face.
pub fn corner_patches(basis: &CornerBasis, ring: &[[f64; 3]]) -> Result<Vec<[f64; 3]>> {
    let ring: Vec<Vec3> = ring.iter().map(|&[x, y, z]| Vec3::new(x, y, z)).collect();
    let patches = basis.patches(&ring).map_err(status)?;
    Ok(patches.into_iter().map(|p| [p.x, p.y, p.z]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    /// A two-ring of a corner of three faces on the plane z = 2 + x / 2 - y / 4: any points do,
    /// the blends' weights sum to one.
    fn flat_ring() -> Vec<[f64; 3]> {
        (0..19)
            .map(|k| {
                let (x, y) = (f64::from(k % 5) - 2.0, f64::from(k / 5) - 1.5);
                [x, y, 2.0 + 0.5 * x - 0.25 * y]
            })
            .collect()
    }

    #[test]
    fn a_flat_two_ring_gives_flat_patches_and_a_place_is_its_blend() {
        let basis = corner_basis(3).unwrap();
        let ring = flat_ring();
        let patches = corner_patches(&basis, &ring).unwrap();
        assert_eq!(patches.len(), 3 * POINTS);
        for p in &patches {
            assert!(
                (p[2] - (2.0 + 0.5 * p[0] - 0.25 * p[1])).abs() < 1e-9,
                "{p:?}"
            );
        }
        // The corner of patch 1, its point (0, 0), is the place (0, 0) on it.
        let at_corner = place_blend(&basis, 1, 0.0, 0.0).unwrap();
        let point = point_blend(&basis, 1, 0).unwrap();
        for (a, b) in at_corner.iter().zip(point) {
            assert!((a - b).abs() < 1e-12);
        }
        assert_eq!(ring_index(3, 4, (1, 0)).unwrap(), 1 + 6);
    }

    #[test]
    fn what_the_corner_does_not_have_is_refused() {
        assert!(matches!(corner_basis(2), Err(Error::Refused { .. })));
        assert!(matches!(corner_basis(13), Err(Error::Refused { .. })));
        let basis = corner_basis(3).unwrap();
        assert!(matches!(
            corner_patches(&basis, &[[0.0; 3]]),
            Err(Error::Refused { .. })
        ));
        assert!(matches!(
            point_blend(&basis, 3, 0),
            Err(Error::Refused { .. })
        ));
        assert!(matches!(
            place_blend(&basis, 3, 0.5, 0.5),
            Err(Error::Refused { .. })
        ));
        assert!(matches!(
            ring_index(3, 0, (3, 3)),
            Err(Error::Refused { .. })
        ));
    }
}
