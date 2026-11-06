//! New Free Form (`scan.free_form`, SurfaceCraft's after QuickSurface's): a surface made by hand on
//! the scan, a cubic net over four corners clicked round its first face, fitted to the scan as it
//! is made (Follow the Scan), or only to its overall shape so it spans openings and ribs (Overall
//! Shape), or left flat over the corners (Off). It is kept as a surface body of its own, so Edit
//! Surface drags its points, snaps them to the scan and refits them, and Edit Net changes its net.

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_scan::free_form;
pub use solidcraft_scan::free_form::Fitting;
pub use solidcraft_scan::merge::{Merged, Seam};
pub use solidcraft_scan::wrap::{Cylinder, Wrap};

use super::length;
use crate::params::bad;
use crate::schema::scan::{ScanFreeFormResult, ScanMergeSurfacesResult, ScanWrapSurfaceResult};
use crate::{EngineError, Result, Session};

/// How a new face meets the scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// The net lies flat over the corners.
    Off,
    /// The face is fitted to the scan under it.
    Follow,
    /// The same, the places far off it left out: it spans openings, dents and ribs.
    Overall,
}

/// A face made, before it is kept: the scan it was fitted to, the surface and how it fits.
#[derive(Clone, Debug)]
pub struct Made {
    pub scan: Option<String>,
    pub surface: NurbsSurface,
    pub places: usize,
    pub met: usize,
    pub kept: usize,
    pub rms: f64,
}

/// The face over `corners`: fitted to the scan `body` (or the selected or only one) unless `fit`
/// is off, places met no farther than `reach` from it when given.
pub fn make(s: &Session, body: Option<&str>, corners: [Vec3; 4], fit: Fit, fitting: Fitting, reach: Option<f64>) -> Result<Made> {
    let cmd = "scan.free_form";
    if corners.iter().any(|c| !c.is_finite()) {
        return Err(bad(cmd, "the corners must be finite points"));
    }
    let count = fitting.count.clamp(free_form::LEAST_POINTS, free_form::MOST_POINTS);
    if fit == Fit::Off {
        let surface = free_form::surface(&free_form::net_over(corners, count), count).ok_or_else(|| bad(cmd, "the corners make no face"))?;
        return Ok(Made { scan: None, surface, places: 0, met: 0, kept: 0, rms: 0.0 });
    }
    let (scan, on) = super::curve::projector(s, body, cmd)?;
    let reach = reach.unwrap_or(f64::INFINITY);
    let snap = |ps: &[Vec3]| ps.iter().map(|p| on.nearest(*p).filter(|(_, d)| *d <= reach).map(|(q, _)| q)).collect::<Vec<_>>();
    let fitting = Fitting { count, overall: fit == Fit::Overall, ..fitting };
    let fitted = free_form::fitted(corners, &fitting, snap)
        .ok_or_else(|| bad(cmd, "no face could be fitted over those corners: click them round a face, not in a line"))?;
    if fitted.met == 0 {
        return Err(bad(cmd, "the scan is nowhere near those corners within the snap distance"));
    }
    Ok(Made { scan: Some(scan), surface: fitted.surface, places: fitted.places, met: fitted.met, kept: fitted.kept, rms: fitted.rms })
}

/// The corners: four points [x, y, z], in order round the face.
fn corners_param(p: &Value, cmd: &str) -> Result<[Vec3; 4]> {
    let list =
        p.get("corners").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`corners` must list four points [x, y, z], in order round the face"))?;
    let points: Vec<Vec3> =
        list.iter().map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, "a corner must be [x, y, z] of numbers"))).collect::<Result<_>>()?;
    <[Vec3; 4]>::try_from(points).map_err(|_| bad(cmd, "`corners` must be four points, in order round the face"))
}

pub(super) fn new_free_form(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.free_form";
    let corners = corners_param(p, cmd)?;
    let fit = match crate::params::str_(p, "fit").unwrap_or("follow") {
        "off" => Fit::Off,
        "follow" => Fit::Follow,
        "overall" => Fit::Overall,
        other => return Err(bad(cmd, format!("`fit` must be off, follow or overall, not `{other}`"))),
    };
    let count = match p.get("count").filter(|v| !v.is_null()) {
        Some(v) => v
            .as_u64()
            .filter(|n| (free_form::LEAST_POINTS as u64..=free_form::MOST_POINTS as u64).contains(n))
            .ok_or_else(|| bad(cmd, "`count` must be a whole number from 4 to 12"))? as usize,
        None => free_form::LEAST_POINTS,
    };
    let stiffness = match p.get("stiffness").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`stiffness` must be a number from 0 to 1"))?,
        None => 0.0,
    };
    let reach = match crate::params::expr(p, "reach") {
        Some(_) => Some(length(s, p, "reach", 1.0, cmd)?),
        None => None,
    };
    let body = crate::params::str_(p, "body").map(str::to_owned);
    let made = make(s, body.as_deref(), corners, fit, Fitting { count, stiffness, overall: false }, reach)?;
    let file = made.scan.as_deref().map_or_else(|| "free form".to_owned(), |scan| format!("{scan} (free form)"));
    let (name, feature) = super::surface::keep_surface(s, &made.surface, "FreeForm", "Free Form", &file, cmd)?;
    let reply = ScanFreeFormResult {
        body: made.scan,
        surface: name,
        controls: made.surface.count(),
        places: made.places,
        met: made.met,
        kept: made.kept,
        rms: made.rms,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// A net wrapped on a tube-like part of a scan, before it is kept: the scan, the cylinder it was
/// laid on, how far the part is from that on average, and the face.
#[derive(Clone, Debug)]
pub struct Wrapped {
    pub scan: String,
    pub cylinder: Cylinder,
    pub off: f64,
    pub made: Made,
}

/// The part's reach round the point when none is given: a fifth of the scan's size.
pub fn default_wrap_reach(s: &Session, body: Option<&str>) -> Result<f64> {
    Ok(super::default_surface_radius(s, body)? * 2.0)
}

/// The net `scan.wrap_surface` would wrap on the part of the scan `body` within `reach` of `point`.
pub fn wrapped(s: &Session, body: Option<&str>, point: Vec3, reach: f64, wrap: Wrap) -> Result<Wrapped> {
    let cmd = "scan.wrap_surface";
    if !point.is_finite() {
        return Err(bad(cmd, "`point` must be a finite point"));
    }
    if !(reach.is_finite() && reach > 0.0) {
        return Err(bad(cmd, "`reach` must be a length above 0"));
    }
    let (name, on) = super::curve::projector(s, body, cmd)?;
    let state = s.model.state();
    let (positions, triangles) =
        state.body(&name).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    let scan = solidcraft_scan::Scan::new(positions, triangles);
    let seed = solidcraft_scan::base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let snap = |ps: &[Vec3], limit: f64| ps.iter().map(|p| on.nearest(*p).filter(|(_, d)| *d <= limit).map(|(q, _)| q)).collect::<Vec<_>>();
    let w = solidcraft_scan::wrap::wrap(&scan, seed, point, reach, &wrap, snap).map_err(|e| bad(cmd, e.to_string()))?;
    let f = w.fitted;
    let made = Made { scan: Some(name.clone()), surface: f.surface, places: f.places, met: f.met, kept: f.kept, rms: f.rms };
    Ok(Wrapped { scan: name, cylinder: w.cylinder, off: w.off, made })
}

pub(super) fn wrap_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.wrap_surface";
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let axis = match crate::params::str_(p, "axis").unwrap_or("fitted") {
        "fitted" => None,
        "x" | "X" => Some(Vec3::X),
        "y" | "Y" => Some(Vec3::Y),
        "z" | "Z" => Some(Vec3::Z),
        other => return Err(bad(cmd, format!("`axis` must be fitted, x, y or z, not `{other}`"))),
    };
    let details = match crate::params::str_(p, "details").unwrap_or("medium") {
        "low" => 0,
        "medium" => 1,
        "fine" => 2,
        other => return Err(bad(cmd, format!("`details` must be low, medium or fine, not `{other}`"))),
    };
    let arc = match crate::params::expr(p, "arc") {
        Some(e) => {
            let e = s.doc.with_design_unit(e.as_str(), solidcraft_doc::expr::Kind::Angle);
            let radians = s.doc.eval(&e, solidcraft_doc::expr::Kind::Angle).map_err(|e| bad(cmd, format!("arc: {e}")))?;
            if !(radians.is_finite() && (0.0..=std::f64::consts::TAU + 1e-9).contains(&radians)) {
                return Err(bad(cmd, "`arc` must be an angle from 0 to 360°"));
            }
            (radians > 0.0).then_some(radians)
        }
        None => None,
    };
    let stiffness = match p.get("stiffness").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`stiffness` must be a number from 0 to 1"))?,
        None => 0.0,
    };
    let body = super::mesh_body_name(s, crate::params::str_(p, "body"), cmd)?;
    let point = point.or_else(|| super::picked_point(s, &body)).ok_or_else(|| bad(cmd, "pick a point on the part of the scan to wrap"))?;
    let reach = match crate::params::expr(p, "reach") {
        Some(_) => length(s, p, "reach", 1.0, cmd)?,
        None => default_wrap_reach(s, Some(&body))?,
    };
    let w = wrapped(s, Some(&body), point, reach, Wrap { axis, details, arc, stiffness })?;
    let (name, feature) = super::surface::keep_surface(s, &w.made.surface, "FreeForm", "Wrap Surface", &format!("{body} (wrap)"), cmd)?;
    let at = |v: Vec3| [v.x, v.y, v.z];
    let reply = ScanWrapSurfaceResult {
        body,
        surface: name,
        controls: w.made.surface.count(),
        point: at(w.cylinder.point),
        axis: at(w.cylinder.axis),
        radius: w.cylinder.radius,
        off: w.off,
        places: w.made.places,
        met: w.made.met,
        rms: w.made.rms,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The surfaces Merge works on: those named, else the selected ones, else every surface in the
/// design whose net can be edited. One alone is merged with itself: a wrap closed at its seam.
pub fn merge_candidates(s: &Session, names: Option<&[String]>, cmd: &str) -> Result<Vec<super::surface::EditableSurface>> {
    if let Some(names) = names {
        let mut seen: Vec<&str> = Vec::new();
        let mut out = Vec::new();
        for name in names {
            if seen.contains(&name.as_str()) {
                continue;
            }
            seen.push(name);
            out.push(super::surface::editable(s, Some(name), cmd)?);
        }
        return Ok(out);
    }
    let mut selected: Vec<String> = Vec::new();
    for x in &s.selection {
        if let crate::Sel::Body { name } | crate::Sel::Face { body: name, .. } | crate::Sel::Edge { body: name, .. } = x
            && !selected.contains(name)
        {
            selected.push(name.clone());
        }
    }
    let chosen: Vec<_> = selected.iter().filter_map(|n| super::surface::editable(s, Some(n), cmd).ok()).collect();
    if !chosen.is_empty() {
        return Ok(chosen);
    }
    let names: Vec<String> = s.model.state().bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.clone()).collect();
    Ok(names.iter().filter_map(|n| super::surface::editable(s, Some(n), cmd).ok()).collect())
}

/// The gap a merge closes when none is given: a tenth of the shortest side of the nets, measured
/// along it (a net wrapped round a part has corners close across its seam).
pub fn default_gap(surfaces: &[NurbsSurface]) -> f64 {
    let shortest =
        surfaces.iter().flat_map(|s| solidcraft_scan::merge::SIDES.map(|side| side.length(s))).filter(|l| *l > 0.0).fold(f64::INFINITY, f64::min);
    if shortest.is_finite() { 0.1 * shortest } else { 1.0 }
}

/// The surfaces as Merge would make them meet, within `gap` (default [`default_gap`]), and the
/// gap used.
pub fn merged(s: &Session, names: Option<&[String]>, gap: Option<f64>) -> Result<(Vec<super::surface::EditableSurface>, Merged, f64)> {
    let cmd = "scan.merge_surfaces";
    let targets = merge_candidates(s, names, cmd)?;
    if targets.is_empty() {
        return Err(bad(cmd, "the design has no surface to merge: make them with New Free Form, Wrap Surface or Fit Surface"));
    }
    let surfaces: Vec<NurbsSurface> = targets.iter().map(|t| t.surface.clone()).collect();
    let gap = gap.unwrap_or_else(|| default_gap(&surfaces));
    if !(gap.is_finite() && gap > 0.0) {
        return Err(bad(cmd, "`gap` must be a length above 0"));
    }
    let made = solidcraft_scan::merge::merge(&surfaces, gap).map_err(|e| bad(cmd, e.to_string()))?;
    Ok((targets, made, gap))
}

/// Why sewing `surfaces` (`what`: "the seam", "the surfaces") failed, in words rather than the
/// kernel's test names: the joined surface folding over itself where its sides were brought
/// together (by Surface Check), else what the kernel's check refused.
pub(super) fn sewing_refusal(what: &str, surfaces: &[&NurbsSurface], kernel: &str, cmd: &str) -> EngineError {
    let trouble = surfaces.iter().map(|f| solidcraft_scan::surface_check::trouble(f)).fold([0; 3], |a, b| [a[0] + b[0], a[1] + b[1], a[2] + b[2]]);
    if trouble.iter().sum::<usize>() > 0 {
        return bad(
            cmd,
            format!(
                "{what} could not be sewn: where the sides were brought together the surface folds over itself ({}); bring the sides closer and running the same way (Edit Surface), or merge with a smaller gap",
                super::surface::trouble_text(trouble)
            ),
        );
    }
    bad(cmd, format!("{what} could not be sewn: {}", kernel_words(kernel)))
}

/// What the kernel's check refused, in words: each kind of failure it names, once.
fn kernel_words(kernel: &str) -> String {
    let kinds = [
        ("NonMonotonic", "the control net doubles back on itself"),
        ("CurveCusp", "an edge curve kinks sharply"),
        ("Bad Gap", "an edge does not lie on the faces it joins"),
        ("Bad Edge loc", "an edge does not lie on the faces it joins"),
        ("ntersect", "the surface passes through itself"),
    ];
    let mut said: Vec<&str> = Vec::new();
    for (name, words) in kinds {
        if kernel.contains(name) && !said.contains(&words) {
            said.push(words);
        }
    }
    if said.is_empty() { "the kernel's check refused the joined surface".to_string() } else { said.join("; ") }
}

pub(super) fn merge_surfaces(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.merge_surfaces";
    let names = match p.get("surfaces").filter(|v| !v.is_null()) {
        Some(v) => Some(
            v.as_array()
                .filter(|a| a.len() <= 64)
                .ok_or_else(|| bad(cmd, "`surfaces` must list up to 64 surface body names"))?
                .iter()
                .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| bad(cmd, "`surfaces` must list surface body names")))
                .collect::<Result<Vec<String>>>()?,
        ),
        None => None,
    };
    let gap = match crate::params::expr(p, "gap") {
        Some(_) => Some(length(s, p, "gap", 1.0, cmd)?),
        None => None,
    };
    let stitch = crate::params::bool_(p, "stitch").unwrap_or(true);
    let (targets, made, _) = merged(s, names.as_deref(), gap)?;
    for (i, (target, surface)) in targets.iter().zip(&made.surfaces).enumerate() {
        // A net closed round on its own seam is sewn there as it is written back.
        match made.seams.iter().find(|m| m.a == i && m.b == i).filter(|_| stitch) {
            Some(seam) => super::surface::write_closed(s, target, surface, seam.a_side, cmd)?,
            None if &target.surface != surface => super::surface::write_back(s, target, surface, cmd)?,
            None => {}
        }
    }
    let bodies: Vec<String> = targets.iter().map(|t| t.body.clone()).collect();
    let run = |s: &mut Session, id: &str, p: Value| -> Result<Value> {
        super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
    };
    let feature = if stitch && bodies.len() > 1 {
        let all: Vec<&NurbsSurface> = made.surfaces.iter().collect();
        Some(
            run(s, "surface.stitch", serde_json::json!({ "bodies": bodies }))
                .map_err(|e| sewing_refusal("the surfaces", &all, &e.to_string(), cmd))?,
        )
    } else {
        None
    };
    let reply = ScanMergeSurfacesResult {
        body: stitch.then(|| bodies[0].clone()),
        surfaces: bodies,
        seams: made.seams.len(),
        gap: made.seams.iter().map(|m| m.gap).fold(0.0, f64::max),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
mod sewing_tests {
    /// The kernel's refusal of a seam on T-LESS 01 (a wrap merged round on itself), in words: no
    /// test numbers, owners or offsets, each kind once.
    #[test]
    fn a_sewing_refusal_is_said_in_words() {
        let kernel = "the operation failed: SMLib stitch: SMLib's check refuses the result: 2 failed tests:\nSmBSplineSurface::Test #5 Bad NonMonotonic CPts: SmBSplineSurface Control Net doubles back on itself probably making a self intersecting surface [owner SmBSplineSurface]\nSmBSplineCurve::Test #14 Bad CurveCusp: 3D BSplineCurve Warning: Extremely small radius of curvature (offset=0.0028879039, tolerance=0.0076947937) [owner SmBSplineCurve]";
        let words = super::kernel_words(kernel);
        assert_eq!(words, "the control net doubles back on itself; an edge curve kinks sharply; the surface passes through itself");
        assert!(!words.contains("Sm") && !words.contains('#'));
        assert_eq!(super::kernel_words("something else"), "the kernel's check refused the joined surface");
    }
}
