//! Flat Template (`scan.flat_template`): a template of a wall from its scan, for cutting a sheet to
//! fit it. The outline is the points picked, laid exactly on the wall's plane (fitted to the scan
//! under the outline with what stands off it left out, or given); nothing else follows the scan.
//! Each part that stands off the plane gets a cut-out, and the scan's own edge is drawn as
//! construction lines on the same plane, so the template can be checked against the scan. It is a
//! sketch on XY with the outline's corner at the origin, so it exports and extrudes like any other.

use serde_json::{Value, json};
use solidcore_record::{Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::chain::{self, Outline, Piece};
use solidcraft_scan::template::{self, Side};

use super::{CommandSpec, mesh};
use crate::params::{bad, bool_, str_};
use crate::schema::scan_template::{ScanFlatTemplateParams, ScanFlatTemplateResult, ScanMakeFlatParams, ScanMakeFlatResult, ScanTemplateCutout};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanFlatTemplateParams> = TypedContract::new();
const MAKE_FLAT_CONTRACT: TypedContract<ScanMakeFlatParams> = TypedContract::new();

/// The sketch the template goes in.
pub const TEMPLATE_NAME: &str = "Wall Template";

/// How far the lines and arcs fitted to a footprint or the scan's edge may stray from it (mm).
const FIT_TOLERANCE: f64 = 0.25;

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.flat_template", "Flat Template", crate::checked!("scan.flat_template", flat_template))
        .at("SCAN", "SKETCH")
        .icon("flat")
        .contract(&CONTRACT)
        .output(output_schema_for::<ScanFlatTemplateResult>)
        .params("body?: the scan (default: the picked, selected or only one); outline: [[x,y,z], …] three or more points in order round the template (laid exactly on the plane; nothing else snaps to the scan); plane?: {point, normal} (default: fitted to the scan under the outline, the parts standing far off it left out); tolerance?: length (how far off the plane a part must stand to be cut out; default four times the wall's noise, at least 1 mm); side?: both|out|in (default both; out is the side the parts stand farthest off); least_area?: mm² (default 25: smaller parts are noise); show_scan_edge?: bool (default true: the scan's edge as construction lines); make_face?: bool (default false: also a flat Free Form face over the template with each cut-out a hole in it); name?: the sketch's (default `Wall Template`): a sketch on a plane through the wall at the outline's corner (the sketch's origin), a cut-out loop for each part standing off the plane, and the scan's edge → body, sketch, point, normal, fitted, kept, under, tolerance, area, cut_area, cutouts[{area, height, side, center, size, at_scan_edge, crosses, clipped}], edge_lengths (the outline's sides, mm), face (the Free Form body, with make_face), found, specks, outside, scan_edge_loops, lines, arcs, circles, shift"),
    CommandSpec::new("scan.make_flat", "Make Flat", crate::checked!("scan.make_flat", make_flat))
        .at("SCAN", "FREEFORM")
        .icon("flat")
        .contract(&MAKE_FLAT_CONTRACT)
        .output(output_schema_for::<ScanMakeFlatResult>)
        .params("surface?: the Free Form (default: the selected one, else the only one); plane?: {point, normal} (default: fitted to its net with the points far off it left out): every control point of every face moved square onto the plane, so the Free Form is exactly flat; one undo step → surface, faces, point, normal, rms, max, feature"),
];

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

fn closed_lines(ring: &[Vec2]) -> Outline {
    let n = ring.len();
    Outline::Chain((0..n).map(|i| Piece::Line { from: ring[i], to: ring[(i + 1) % n] }).collect())
}

fn vec3_of(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// A template worked out, before anything is drawn: the plane used and how it was found, and the
/// template itself.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub body: String,
    pub made: template::Template,
    pub point: Vec3,
    pub tolerance: f64,
    /// The plane was fitted to the scan (rather than given), its points and those under the outline.
    pub fitted: bool,
    pub kept: usize,
    pub under: usize,
    pub name: String,
    pub show_scan_edge: bool,
}

/// The template `p` asks for (see `scan.flat_template`), worked out without changing the design:
/// what the dialog shows over the scan before OK.
pub fn prepare(s: &Session, p: &Value, cmd: &str) -> Result<Prepared> {
    let outline: Vec<Vec3> = p
        .get("outline")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(cmd, "`outline` must list three or more points [x, y, z], in order round the template"))?
        .iter()
        .map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, "an outline point must be [x, y, z] of numbers")))
        .collect::<Result<_>>()?;
    if outline.len() < 3 || outline.len() > template::MOST_OUTLINE {
        return Err(bad(cmd, format!("`outline` must have three to {} points", template::MOST_OUTLINE)));
    }
    let side = match str_(p, "side").unwrap_or("both") {
        "both" => Side::Both,
        "out" => Side::Out,
        "in" => Side::In,
        other => return Err(bad(cmd, format!("`side` must be both, out or in, not `{other}`"))),
    };
    let least_area = match p.get("least_area").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|a| a.is_finite() && *a >= 0.0).ok_or_else(|| bad(cmd, "`least_area` must be an area (mm²) of 0 or more"))?,
        None => template::LEAST_AREA,
    };
    let given_plane = match p.get("plane").filter(|v| !v.is_null()) {
        Some(v) => {
            let point = v.get("point").and_then(crate::params::vec3);
            let normal = v.get("normal").and_then(crate::params::vec3).and_then(|n| n.normalized());
            match (point, normal) {
                (Some(point), Some(normal)) => Some((point, normal)),
                _ => return Err(bad(cmd, "`plane` needs `point` and a `normal` that has a direction, each [x, y, z] of numbers")),
            }
        }
        None => None,
    };
    let asked = match crate::params::expr(p, "tolerance") {
        Some(_) => Some(super::length(s, p, "tolerance", 1.0, cmd)?),
        None => None,
    };
    let name = str_(p, "name").filter(|n| !n.trim().is_empty()).unwrap_or(TEMPLATE_NAME).to_owned();
    let on = mesh(s, p, cmd)?;
    let scan = on.scan();
    let (point, normal, tolerance, fitted, kept, under) = match given_plane {
        Some((point, normal)) => (point, normal, asked.unwrap_or(1.0), false, 0, 0),
        None => {
            let wall = template::wall(&scan, &outline).ok_or_else(|| {
                bad(cmd, "the scan under the outline makes no plane: pick the outline over the wall, not in a line, or give `plane`")
            })?;
            (wall.point, wall.normal, asked.unwrap_or(wall.tolerance), true, wall.kept, wall.under)
        }
    };
    let made = template::template(&scan, point, normal, &outline, tolerance, side, least_area).map_err(|e| bad(cmd, e.to_string()))?;
    Ok(Prepared { body: on.body, made, point, tolerance, fitted, kept, under, name, show_scan_edge: bool_(p, "show_scan_edge").unwrap_or(true) })
}

/// A point inside the outline and clear of every cut-out, as far from their edges as a grid of
/// candidates finds: where the trim keeps the face.
fn clear_point(made: &template::Template) -> Option<Vec2> {
    use solidcraft_scan::profile::contains;
    let (lo, hi) = made.outline.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    let rings: Vec<&Vec<Vec2>> = std::iter::once(&made.outline).chain(made.cutouts.iter().map(|c| &c.ring)).collect();
    let mut best: Option<(f64, Vec2)> = None;
    const N: usize = 41;
    for j in 0..N {
        for i in 0..N {
            let q = Vec2::new(lo.x + (hi.x - lo.x) * (i as f64 + 0.5) / N as f64, lo.y + (hi.y - lo.y) * (j as f64 + 0.5) / N as f64);
            if !contains(&made.outline, q) || made.cutouts.iter().any(|c| contains(&c.ring, q)) {
                continue;
            }
            let room = rings.iter().flat_map(|r| r.iter()).map(|v| v.dist(q)).fold(f64::MAX, f64::min);
            if best.is_none_or(|(r, _)| room > r) {
                best = Some((room, q));
            }
        }
    }
    best.map(|(_, q)| q)
}

/// The flat Free Form face over the template's box, trimmed to the outline (when it is not the
/// box) and with each cut-out trimmed out: its body's name.
#[allow(clippy::too_many_arguments)]
fn template_face(
    s: &mut Session,
    scan: &str,
    made: &template::Template,
    ids: &[String],
    after_outline: usize,
    after_cuts: usize,
    sketch: u64,
    cmd: &str,
) -> Result<String> {
    let (lo, hi) = made.outline.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    let corners = [Vec2::new(lo.x, lo.y), Vec2::new(hi.x, lo.y), Vec2::new(hi.x, hi.y), Vec2::new(lo.x, hi.y)].map(|q| made.at(q));
    let normal = made.frame.x.cross(made.frame.y);
    let surface = solidcraft_scan::free_form::flat_over(corners, solidcraft_scan::free_form::LEAST_POINTS, made.frame.origin, normal)
        .ok_or_else(|| bad(cmd, "the template makes no face"))?;
    let is_box = (made.area - (hi.x - lo.x) * (hi.y - lo.y)).abs() <= 1e-9 * made.area.abs().max(1.0);
    let (first, last) = if is_box { (after_outline, after_cuts) } else { (0, after_cuts) };
    let trims: Vec<&String> = ids.iter().skip(first).take(last.saturating_sub(first)).collect();
    let (name, _) = super::surface::keep_surface(s, &surface, "FreeForm", "Free Form", &format!("{scan} (flat template)"), cmd)?;
    if !trims.is_empty() {
        let keep = clear_point(made).ok_or_else(|| bad(cmd, "no part of the template is left clear of the cut-outs"))?;
        let keep = made.at(keep);
        run(s, "surface.trim", json!({"body": name, "curves": {"sketch": sketch, "curves": trims}, "keep": [[keep.x, keep.y, keep.z]]}))?;
    }
    Ok(name)
}

fn flat_template(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.flat_template";
    if s.active_sketch.is_some() {
        return Err(bad(cmd, "finish the sketch being edited first"));
    }
    let Prepared { body, made, point, tolerance, fitted, kept, under, name, show_scan_edge } = prepare(s, p, cmd)?;

    // The outline's corner at the origin.
    let low = made.outline.iter().fold(Vec2::new(f64::MAX, f64::MAX), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
    let shift = Vec2::new(-low.x, -low.y);
    let moved = |ring: &[Vec2]| -> Vec<Vec2> { ring.iter().map(|q| *q + shift).collect() };
    let fit = |ring: &[Vec2]| chain::chain(&moved(ring), true, FIT_TOLERANCE);

    let edge_outlines: Vec<Outline> = if show_scan_edge { made.scan_edge.iter().filter_map(|l| fit(l)).collect() } else { Vec::new() };
    let cut_outlines: Vec<Outline> = made.cutouts.iter().map(|c| fit(&c.ring).unwrap_or_else(|| closed_lines(&moved(&c.ring)))).collect();

    // The sketch lies on the wall: a plane through the outline's corner, its axes the template's.
    let corner = made.at(low);
    let at = |q: Vec3| json!({ "point": [q.x, q.y, q.z] });
    let plane_name = (0..)
        .map(|i| if i == 0 { format!("{name} Plane") } else { format!("{name} Plane {i}") })
        .find(|n| s.doc.find_feature(n).is_none())
        .unwrap_or_default();
    run(
        s,
        "construct.plane.three_points",
        json!({"a": at(corner), "b": at(corner + made.frame.x), "c": at(corner + made.frame.y), "name": plane_name}),
    )?;
    let created = run(s, "sketch.create", json!({"plane": plane_name, "name": name}))?;
    let sketch = created.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the template's sketch was not made"))?;
    let mut added = super::section::Added::default();
    let counts = std::cell::Cell::new((0usize, 0usize));
    let drawn = super::super::sketch_store::update(s, sketch, |_, _, sk| {
        super::section::draw(sk, &[(closed_lines(&moved(&made.outline)), true)], &mut added)?;
        let after_outline = sk.curves.len();
        let cuts: Vec<(Outline, bool)> = cut_outlines.iter().cloned().map(|o| (o, true)).collect();
        super::section::draw(sk, &cuts, &mut added)?;
        counts.set((after_outline, sk.curves.len()));
        // The scan's edge is only there to look at: construction geometry, never a profile.
        let before = sk.curves.len();
        let edge: Vec<(Outline, bool)> = edge_outlines.iter().cloned().map(|o| (o, true)).collect();
        super::section::draw(sk, &edge, &mut added)?;
        for curve in sk.curves.iter_mut().skip(before) {
            curve.construction = true;
        }
        Ok(())
    });
    if let Err(e) = drawn {
        run(s, "sketch.cancel", json!({}))?;
        return Err(e);
    }
    run(s, "sketch.finish", json!({}))?;

    // A flat Free Form face over the template with the cut-outs trimmed out of it.
    let face = if bool_(p, "make_face").unwrap_or(false) {
        let (after_outline, after_cuts) = counts.get();
        let ids: Vec<String> =
            super::super::sketch_store::read(&s.doc, sketch).map(|sk| sk.curves.iter().map(|c| c.id.clone()).collect()).unwrap_or_default();
        Some(template_face(s, &body, &made, &ids, after_outline, after_cuts, sketch, cmd)?)
    } else {
        None
    };

    let cutouts = made
        .cutouts
        .iter()
        .map(|c| {
            let (lo, hi) = c.ring.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
                (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
            });
            ScanTemplateCutout {
                area: c.area,
                height: c.height,
                side: if c.side == Side::In { "in" } else { "out" }.into(),
                center: vec3_of(made.at((lo + hi) / 2.0)),
                size: [hi.x - lo.x, hi.y - lo.y],
                at_scan_edge: c.at_scan_edge,
                crosses: c.crosses,
                clipped: c.clipped,
            }
        })
        .collect();
    let normal = made.frame.x.cross(made.frame.y);
    let edge_lengths = made.outline.iter().zip(made.outline.iter().cycle().skip(1)).map(|(a, b)| a.dist(*b)).collect();
    let reply = ScanFlatTemplateResult {
        body,
        sketch,
        point: vec3_of(point),
        normal: vec3_of(normal),
        fitted,
        kept,
        under,
        tolerance,
        area: made.area,
        cut_area: made.cut_area,
        cutouts,
        found: made.found,
        specks: made.specks,
        outside: made.outside,
        scan_edge_loops: edge_outlines.len(),
        lines: added.lines,
        arcs: added.arcs,
        circles: added.circles,
        shift: [shift.x, shift.y],
        face,
        edge_lengths,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Every control point of the Free Form moved square onto one plane: given, or fitted to the net
/// with the points far off it left out.
fn make_flat(s: &mut Session, p: &Value) -> Result<Value> {
    use solidcraft_scan::surface_edit::{points, with_points};
    let cmd = "scan.make_flat";
    let given = match p.get("plane").filter(|v| !v.is_null()) {
        Some(v) => {
            let point = v.get("point").and_then(crate::params::vec3);
            let normal = v.get("normal").and_then(crate::params::vec3).and_then(|n| n.normalized());
            match (point, normal) {
                (Some(point), Some(normal)) => Some((point, normal)),
                _ => return Err(bad(cmd, "`plane` needs `point` and a `normal` that has a direction, each [x, y, z] of numbers")),
            }
        }
        None => None,
    };
    let target = super::surface::editable(s, str_(p, "surface"), cmd)?;
    let net: Vec<Vec3> = target.faces.iter().flat_map(points).collect();
    let (point, normal) = match given {
        Some(plane) => plane,
        None => {
            let (point, normal, _) =
                solidcraft_scan::free_form::flat_plane(&net).ok_or_else(|| bad(cmd, "the Free Form's points make no plane: give `plane`"))?;
            (point, normal)
        }
    };
    let off: Vec<f64> = net.iter().map(|q| (*q - point).dot(normal)).collect();
    let faces: Vec<_> = target
        .faces
        .iter()
        .map(|f| {
            let flat: Vec<Vec3> = points(f).iter().map(|q| *q - normal * (*q - point).dot(normal)).collect();
            with_points(f, &flat)
        })
        .collect::<Option<_>>()
        .ok_or_else(|| bad(cmd, "the Free Form cannot be laid on that plane: its points would fold into a line"))?;
    super::surface::write_faces(s, &target, &faces, cmd)?;
    let joins = super::network_edit::joins_for(s, &target, &faces, cmd)?;
    super::network_edit::keep(s, &target, &faces, &joins, None);
    let rms = if off.is_empty() { 0.0 } else { (off.iter().map(|d| d * d).sum::<f64>() / off.len() as f64).sqrt() };
    let reply = ScanMakeFlatResult {
        surface: target.body,
        faces: faces.len(),
        point: vec3_of(point),
        normal: vec3_of(normal),
        rms,
        max: off.iter().fold(0.0, |m, d| m.max(d.abs())),
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
