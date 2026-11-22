//! Surface bodies read as NURBS nets, and a Free Form network thickened from its own nets.
//!
//! SMLib thickens a sheet by offsetting each face on its own and meeting the offsets; where
//! doubly curved B-spline faces meet, the offsets miss each other at the corners by a hair more
//! than its check takes, so a Free Form network of several faces is refused (after seconds of
//! trying). Thicken asks [`thickened_network`] first for such a body, which makes the same solid
//! from the network's nets instead (`solidcraft_scan::network::thickened`): the network moved
//! along its normals as one, its faces sharing their sides exactly, with ruled strips on its open
//! sides, sewn shut.

use solidcore_record::Vec3;
use solidcore_record::brep as b;
use solidcraft_geom::bspline::{HPoint, NurbsSurface};
use solidcraft_kernel as kernel;

/// A B-spline record surface (rational or not) as a NURBS surface, on the same (u, v); `None` for
/// any other kind of surface.
pub fn nurbs_of_surface(surface: &b::Surface) -> Option<NurbsSurface> {
    let (u_knots, v_knots, rows): (&Vec<f64>, &Vec<f64>, Vec<Vec<HPoint>>) = match surface {
        b::Surface::Spline { u_knots, v_knots, points } => {
            (u_knots, v_knots, points.iter().map(|row| row.iter().map(|p| HPoint::weighted(Vec3::new(p[0], p[1], p[2]), 1.0)).collect()).collect())
        }
        b::Surface::RationalSpline { u_knots, v_knots, homogeneous } => {
            (u_knots, v_knots, homogeneous.iter().map(|row| row.iter().map(|h| HPoint { x: h[0], y: h[1], z: h[2], w: h[3] }).collect()).collect())
        }
        _ => return None,
    };
    // The record's rows run along u (`rows[i][j]`, i along u); the surface's points go u fastest.
    let nu = rows.len();
    let nv = rows.first()?.len();
    if nu == 0 || nv == 0 || rows.iter().any(|r| r.len() != nv) {
        return None;
    }
    let degree = [u_knots.len().checked_sub(nu + 1)?, v_knots.len().checked_sub(nv + 1)?];
    let ctrl = (0..nv).flat_map(|j| rows.iter().map(move |row| row[j])).collect();
    NurbsSurface::from_homogeneous(degree, [u_knots.clone(), v_knots.clone()], ctrl).ok()
}

/// Whether the first face of `r` is used on its surface's own side (its normal), as the shells
/// that hold it say; true for a face no shell names.
fn first_face_forward(r: &b::Brep) -> bool {
    for s in &r.shells {
        if let b::Shell::Faces { faces, .. } = s
            && let Some(f) = faces.iter().find(|f| f.face == 0)
        {
            return f.forward;
        }
    }
    true
}

/// `body`, a Free Form network of several faces (cubic B-splines sharing whole sides, with an
/// open edge), thickened by `thickness` into a closed solid (by half each way when `symmetric`),
/// along the normal its first face is used with (the way the kernel's Thicken grows a sheet whose
/// faces agree), the other faces turned to agree with it. `None` for any other body, or when the
/// faces made do not sew into a solid.
pub fn thickened_network(body: &kernel::Body, thickness: f64, symmetric: bool) -> Option<kernel::Body> {
    if body.is_mesh() || !thickness.is_finite() || thickness == 0.0 {
        return None;
    }
    let record = kernel::brep_record(body).ok()?;
    let faces: Vec<NurbsSurface> = record.faces.iter().map(|f| nurbs_of_surface(&f.surface)).collect::<Option<_>>()?;
    // A network: several cubic faces (`joins` refuses any other) that share sides.
    if faces.len() < 2 || solidcraft_scan::network::joins(&faces).ok().is_none_or(|j| j.seams.is_empty()) {
        return None;
    }
    let t = if first_face_forward(&record) { thickness } else { -thickness };
    let (near, far) = if symmetric { (-0.5 * t, 0.5 * t) } else { (0.0, t) };
    let nets = solidcraft_scan::network::thickened(&faces, near, far).ok()?;
    let sheets: Vec<kernel::Body> = nets.iter().map(kernel::nurbs_sheet).collect::<kernel::Result<_>>().ok()?;
    let refs: Vec<&kernel::Body> = sheets.iter().collect();
    let sewn = kernel::stitch(&refs, 0.0).ok()?;
    sewn.volume_at(0.0).filter(|v| v.is_finite() && *v > 0.0).map(|_| sewn)
}
