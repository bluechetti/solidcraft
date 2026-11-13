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
pub use solidcraft_scan::panel;
use solidcraft_scan::template::{self, Side};
use solidcraft_scan::wall_outline;

use super::{CommandSpec, mesh};
use crate::params::{bad, bool_, str_};
use crate::schema::scan_template::{
    PanelChoices, PanelCutoutProposal, PanelEdgesOut, PanelHoleProposal, PanelMade, PanelSeamProposal, ScanFlatTemplateParams,
    ScanFlatTemplateResult, ScanMakeFlatParams, ScanMakeFlatResult, ScanPanelProposalsParams, ScanPanelProposalsResult, ScanTemplateCutout,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanFlatTemplateParams> = TypedContract::new();
const MAKE_FLAT_CONTRACT: TypedContract<ScanMakeFlatParams> = TypedContract::new();
const PROPOSALS_CONTRACT: TypedContract<ScanPanelProposalsParams> = TypedContract::new();

/// The default width of tears bridged in the scan's edge for a whole outline (mm).
pub const GAP: f64 = wall_outline::GAP;

/// The default width smoothed away in a whole outline (mm).
pub const SMOOTH: f64 = wall_outline::SMOOTH;

/// The default grid a whole outline's corners are rounded to (mm).
pub const ROUND: f64 = wall_outline::ROUND;

/// The default width of the fingers cut off a whole outline (mm).
pub const FINGER: f64 = wall_outline::FINGER;

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
        .params("body?: the scan (default: the picked, selected or only one); outline: [[x,y,z], …] three or more points in order round the template (laid exactly on the plane; nothing else snaps to the scan), or `scan`: the whole wall's outline found from the scan, on the plane it rests on (its outermost large flat face), each part standing off it a notch in the outline or a cut-out (side defaults to out, show_scan_edge to false); gap?: length (with outline scan: tears in the scan's edge narrower than this bridged; default 50 mm); step?: length (with outline scan: the grid it is found on, how true it is; default 2 mm); smooth?: length (with outline scan: notches, cut-outs and slivers narrower than this smoothed away, the template going over a cable or bracket that thin; default 100 mm, 0 keeps them); round?: length (with outline scan: the template tidied, sides nearly level or plumb made so, those nearly on one line put on it, corners rounded to this grid from its corner, no side added or taken away; default 5 mm, 0 as found); finger?: length (with outline scan: fingers of the template narrower than this reaching over 100 mm in between parts standing out are cut off with them; default 300 mm, 0 keeps them); cover?: [[x,y,z], …] (with outline scan, up to 100: at each, the raised part there, as far as it stays within 15 mm of its height there, is covered rather than cut round — a plateau the panel sits over); plane?: {point, normal} (default: fitted to the scan under the outline, the parts standing far off it left out); tolerance?: length (how far off the plane a part must stand to be cut out; default four times the wall's noise, at least 1 mm); side?: both|out|in (default both; out is the side the parts stand farthest off); least_area?: mm² (default 25: smaller parts are noise); show_scan_edge?: bool (default true: the scan's edge as construction lines); panel?: {sheet?: [long, short] mm (default [2440, 1220]), up?: [x, y, z] (which way is up along the wall: the panel turned so its bottom edge is the floor), turn?: 0-3 (quarter turns after that), edges?: {left?, right?, bottom?, top?} (mm from the outline's corner), drop_cutouts?: [n], drop_seams?: [n], drop_holes?: [n], move_seams?: [{seam, by}], holes?: bool} (with outline scan: a straight panel cut from sheets: straight edges, the cut-outs they need, seams along the frame's rows of holes as few as fit the sheet, and circles for the holes near the edges and seams; each kept unless dropped, numbered as `scan.panel_proposals` lists them); make_face?: bool (default false, not with panel: also a flat Free Form face over the template with each cut-out a hole in it, or a planar patch of the template's region when the kernel cannot trim the Free Form); name?: the sketch's (default `Wall Template`): a sketch on a plane through the wall at the outline's corner (the sketch's origin), a cut-out loop for each part standing off the plane, and the scan's edge → body, sketch, point, normal, fitted, kept, under, tolerance, area, cut_area, cutouts[{area, height, side, center, size, at_scan_edge, crosses, clipped}], edge_lengths (the outline's sides, mm), step (with outline scan: the cell used), moved (with round: the most a corner moved), face (the face's body, with make_face), face_kind (free_form or planar), found, specks, outside, scan_edge_loops, lines, arcs, circles, shift"),
    CommandSpec::new("scan.panel_proposals", "Panel Proposals", crate::checked!("scan.panel_proposals", panel_proposals))
        .contract(&PROPOSALS_CONTRACT)
        .output(output_schema_for::<ScanPanelProposalsResult>)
        .params("the whole wall's outline found as for `scan.flat_template` with outline scan (body?, plane?, tolerance?, side?, least_area?, gap?, step?, smooth?, round?, finger?, cover?), and panel?: the sheet, edges and what is dropped or moved: lists what the straight panel would have, numbered, without changing the design → body, origin, x_axis, y_axis, normal (the wall's frame: 0, 0 is the outline's bottom-left corner), edges {left, right, bottom, top}, cutouts[{notch, edges, depth, area, size, center, ring}], seams[{upright, between_rows, from, to, from_point, to_point}], holes[{at, center, diameter, behind}], holes_found, flattened, flattened_depth, sheet, pieces (width × height, top row first), over (how far the longest piece is past the sheet), outline"),
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
    /// With the outline found from the scan: the grid's cell size (mm).
    pub step: Option<f64>,
    /// With the outline found from the scan and rounded: the most a corner moved (mm).
    pub moved: Option<f64>,
    /// With `panel`: the frame's holes found in the scan.
    pub holes: Vec<panel::Hole>,
}

/// The template `p` asks for (see `scan.flat_template`), worked out without changing the design:
/// what the dialog shows over the scan before OK.
pub fn prepare(s: &Session, p: &Value, cmd: &str) -> Result<Prepared> {
    let from_scan = p.get("outline").and_then(Value::as_str) == Some("scan");
    let outline: Vec<Vec3> = if from_scan {
        Vec::new()
    } else {
        p.get("outline")
            .and_then(Value::as_array)
            .ok_or_else(|| bad(cmd, "`outline` must list three or more points [x, y, z], in order round the template, or be `scan` for the whole outline found from the scan"))?
            .iter()
            .map(|v| crate::params::vec3(v).ok_or_else(|| bad(cmd, "an outline point must be [x, y, z] of numbers")))
            .collect::<Result<_>>()?
    };
    if !from_scan && p.get("panel").is_some_and(|v| !v.is_null()) {
        return Err(bad(cmd, "`panel` needs `outline: \"scan\"`: the straight panel is made from the whole wall's outline"));
    }
    if !from_scan && (outline.len() < 3 || outline.len() > template::MOST_OUTLINE) {
        return Err(bad(cmd, format!("`outline` must have three to {} points", template::MOST_OUTLINE)));
    }
    let side = match str_(p, "side") {
        None if from_scan => Side::Out,
        None | Some("both") => Side::Both,
        Some("out") => Side::Out,
        Some("in") => Side::In,
        Some(other) => return Err(bad(cmd, format!("`side` must be both, out or in, not `{other}`"))),
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
    let show_scan_edge = bool_(p, "show_scan_edge").unwrap_or(!from_scan);
    let on = mesh(s, p, cmd)?;
    let scan = on.scan();
    if from_scan {
        let gap = length_or_zero(s, p, "gap", wall_outline::GAP, cmd)?;
        let step = match crate::params::expr(p, "step") {
            Some(_) => super::length(s, p, "step", wall_outline::STEP, cmd)?,
            None => wall_outline::STEP,
        };
        if !(step.is_finite() && step > 0.0) {
            return Err(bad(cmd, "`step` must be a length above 0"));
        }
        let smooth = length_or_zero(s, p, "smooth", wall_outline::SMOOTH, cmd)?;
        let round = length_or_zero(s, p, "round", wall_outline::ROUND, cmd)?;
        let finger = length_or_zero(s, p, "finger", wall_outline::FINGER, cmd)?;
        let options = wall_outline::Options { gap, step, smooth, least_area, round, finger, edge: show_scan_edge };
        let cover: Vec<Vec3> = match p.get("cover").filter(|v| !v.is_null()) {
            None => Vec::new(),
            Some(v) => v
                .as_array()
                .filter(|a| a.len() <= wall_outline::MOST_COVER)
                .ok_or_else(|| bad(cmd, format!("`cover` must list up to {} points [x, y, z]", wall_outline::MOST_COVER)))?
                .iter()
                .map(|q| crate::params::vec3(q).ok_or_else(|| bad(cmd, "a `cover` point must be [x, y, z] of numbers")))
                .collect::<Result<_>>()?,
        };
        let (made, how) = wall_outline::template_covering(&scan, given_plane, asked, side, options, &cover).map_err(|e| bad(cmd, e.to_string()))?;
        let holes = match panel_choices(p, cmd)? {
            Some(c) if c.holes.unwrap_or(true) => panel::frame_holes(&scan, &made.frame),
            _ => Vec::new(),
        };
        return Ok(Prepared {
            body: on.body,
            made,
            point: how.point,
            tolerance: how.tolerance,
            fitted: how.fitted,
            kept: how.kept,
            under: how.under,
            name,
            show_scan_edge,
            step: Some(how.cell),
            moved: (round > 0.0).then_some(how.moved),
            holes,
        });
    }
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
    Ok(Prepared { body: on.body, made, point, tolerance, fitted, kept, under, name, show_scan_edge, step: None, moved: None, holes: Vec::new() })
}

/// A length parameter in mm that may be 0, or `default` when not given.
fn length_or_zero(s: &Session, p: &Value, key: &str, default: f64, cmd: &str) -> Result<f64> {
    let Some(e) = crate::params::expr(p, key) else { return Ok(default) };
    let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("{key}: {e}")))?;
    if !(v.is_finite() && v >= 0.0) {
        return Err(bad(cmd, format!("`{key}` must be a length of 0 or more")));
    }
    Ok(v)
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

/// How much larger than the outline the Free Form sheet is made before it is trimmed (mm, and a
/// share of its size): with no room round it, the outline's sides along the sheet's edge leave
/// slivers too thin for the kernel to find a point inside, and the trim fails.
const SHEET_MARGIN: f64 = 20.0;
const SHEET_MARGIN_SHARE: f64 = 0.02;

/// The face over the template: a flat Free Form over the template's box (grown a margin round it),
/// trimmed to the outline (when it is not the box) and with each cut-out trimmed out; or, when the
/// kernel cannot trim it (a piece too thin to find a point in), a planar patch of the sketch's
/// region inside the outline less the cut-outs. Its body's name, and which it is (`free_form`,
/// `planar`).
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
) -> Result<(String, &'static str)> {
    let (lo, hi) = made.outline.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
        (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
    });
    let is_box = (made.area - (hi.x - lo.x) * (hi.y - lo.y)).abs() <= 1e-9 * made.area.abs().max(1.0);
    let grow = if is_box { 0.0 } else { SHEET_MARGIN.max(SHEET_MARGIN_SHARE * (hi.x - lo.x).max(hi.y - lo.y)) };
    let (lo, hi) = (Vec2::new(lo.x - grow, lo.y - grow), Vec2::new(hi.x + grow, hi.y + grow));
    let corners = [Vec2::new(lo.x, lo.y), Vec2::new(hi.x, lo.y), Vec2::new(hi.x, hi.y), Vec2::new(lo.x, hi.y)].map(|q| made.at(q));
    let normal = made.frame.x.cross(made.frame.y);
    let surface = solidcraft_scan::free_form::flat_over(corners, solidcraft_scan::free_form::LEAST_POINTS, made.frame.origin, normal)
        .ok_or_else(|| bad(cmd, "the template makes no face"))?;
    let (first, last) = if is_box { (after_outline, after_cuts) } else { (0, after_cuts) };
    let trims: Vec<&String> = ids.iter().skip(first).take(last.saturating_sub(first)).collect();
    // The design before the Free Form, to go back to when its trim fails.
    let before = (s.doc.clone(), s.active_sketch);
    let (name, _) = super::surface::keep_surface(s, &surface, "FreeForm", "Free Form", &format!("{scan} (flat template)"), None, cmd)?;
    if trims.is_empty() {
        return Ok((name, "free_form"));
    }
    let keep = clear_point(made).ok_or_else(|| bad(cmd, "no part of the template is left clear of the cut-outs"))?;
    let keep = made.at(keep);
    if run(s, "surface.trim", json!({"body": name, "curves": {"sketch": sketch, "curves": trims}, "keep": [[keep.x, keep.y, keep.z]]})).is_ok() {
        return Ok((name, "free_form"));
    }
    // The kernel could not trim it: the Free Form taken away again, and the template's region
    // patched flat instead.
    (s.doc, s.active_sketch) = before;
    s.refresh();
    Ok((planar_face(s, scan, ids, after_outline, sketch, cmd)?, "planar"))
}

/// A planar patch of the template sketch's region inside the outline (the profile whose outer loop
/// is exactly the outline's curves, the first `after_outline` of `ids`; the cut-outs are its
/// holes): its body's name.
pub(crate) fn planar_face(s: &mut Session, scan: &str, ids: &[String], after_outline: usize, sketch: u64, cmd: &str) -> Result<String> {
    let outline: Vec<&String> = ids.iter().take(after_outline).collect();
    let patched = run(s, "surface.patch", json!({"sketch": sketch, "profiles": [outline], "body_name": format!("{scan} (flat template)")}))?;
    patched
        .get("created_bodies")
        .and_then(Value::as_array)
        .and_then(|b| b.first())
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| bad(cmd, "the template's face was not made"))
}

/// The `panel` asked for, if any.
fn panel_choices(p: &Value, cmd: &str) -> Result<Option<PanelChoices>> {
    match p.get("panel").filter(|v| !v.is_null()) {
        None => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(PanelChoices::default())),
        Some(Value::Bool(false)) => Ok(None),
        Some(v) => serde_json::from_value(v.clone()).map(Some).map_err(|e| bad(cmd, format!("`panel`: {e}"))),
    }
}

/// A straight panel worked out, before anything is drawn: the template it is made from, what is
/// proposed, and where the outline's corner is (the wall's 0, 0, in the template's coordinates).
#[derive(Clone, Debug)]
pub struct PanelPrepared {
    pub prepared: Prepared,
    pub proposal: panel::Proposal,
    pub base: Vec2,
}

impl PanelPrepared {
    /// A point of the panel (template coordinates) in space.
    pub fn at(&self, q: Vec2) -> Vec3 {
        self.prepared.made.at(q)
    }
    /// A point of the panel in mm from the outline's corner.
    pub fn wall(&self, q: Vec2) -> [f64; 2] {
        [q.x - self.base.x, q.y - self.base.y]
    }
}

/// What is kept of a proposal: a mark for each cut-out, seam and hole, and the seams as moved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelKeep {
    pub cuts: Vec<bool>,
    pub seams: Vec<bool>,
    pub holes: Vec<bool>,
    pub moved: Vec<panel::Seam>,
}

/// The straight panel `p` asks for (`outline: "scan"` and `panel`), proposed but not drawn.
pub fn prepare_panel(s: &Session, p: &Value, cmd: &str) -> Result<PanelPrepared> {
    if p.get("outline").and_then(Value::as_str) != Some("scan") {
        return Err(bad(cmd, "a straight panel is made from the whole wall's outline: give `outline: \"scan\"`"));
    }
    let choices = panel_choices(p, cmd)?.unwrap_or_default();
    let mut prepared = prepare(s, p, cmd)?;
    let mut quarters = 0u8;
    if let Some(up) = p.get("panel").and_then(|v| v.get("up")).filter(|v| !v.is_null()) {
        let up = crate::params::vec3(up).and_then(|u| u.normalized()).ok_or_else(|| bad(cmd, "`panel.up` must be a direction [x, y, z]"))?;
        quarters = panel::quarters_for(&prepared.made, up);
    }
    match choices.turn {
        Some(t) if t > 3 => return Err(bad(cmd, "`panel.turn` must be 0 to 3 quarter turns")),
        Some(t) => quarters = (quarters + t) % 4,
        None => {}
    }
    if quarters > 0 {
        let (made, holes) = panel::turned(&prepared.made, &prepared.holes, quarters);
        prepared.made = made;
        prepared.holes = holes;
    }
    let made = &prepared.made;
    let base = made.outline.iter().fold(Vec2::new(f64::MAX, f64::MAX), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
    let sheet = choices.sheet.unwrap_or(panel::SHEET);
    if !sheet.iter().all(|v| v.is_finite() && *v >= 100.0 && *v <= 100_000.0) {
        return Err(bad(cmd, "`panel.sheet` must be [long, short], each 100 mm to 100 m"));
    }
    let round = p.get("round").and_then(Value::as_f64).filter(|r| r.is_finite() && *r >= 0.0).unwrap_or(wall_outline::ROUND);
    let edges = match &choices.edges {
        None => None,
        Some(e) => {
            let found = panel::edges_for(&made.outline, round).ok_or_else(|| bad(cmd, "the outline gives no straight edges"))?;
            let given = |v: Option<f64>, base: f64, default: f64| -> Result<f64> {
                match v {
                    None => Ok(default),
                    Some(v) if v.is_finite() && v.abs() <= 1.0e6 => Ok(v + base),
                    Some(_) => Err(bad(cmd, "`panel.edges` must be lengths in mm")),
                }
            };
            Some(panel::Edges {
                left: given(e.left, base.x, found.left)?,
                right: given(e.right, base.x, found.right)?,
                bottom: given(e.bottom, base.y, found.bottom)?,
                top: given(e.top, base.y, found.top)?,
            })
        }
    };
    let proposal = panel::propose(made, &prepared.holes, edges, sheet, round).ok_or_else(|| {
        bad(cmd, "no straight panel fits: the edges must leave some of the wall's outline between them (left below right, bottom below top)")
    })?;
    Ok(PanelPrepared { prepared, proposal, base })
}

/// What `p`'s `panel` keeps of `proposal`: every number dropped must be one it has.
pub fn panel_keep(p: &Value, proposal: &panel::Proposal, cmd: &str) -> Result<PanelKeep> {
    let choices = panel_choices(p, cmd)?.unwrap_or_default();
    let marks = |drop: &Option<Vec<usize>>, n: usize, what: &str| -> Result<Vec<bool>> {
        let mut keep = vec![true; n];
        for &i in drop.iter().flatten() {
            *keep.get_mut(i).ok_or_else(|| bad(cmd, format!("there is no {what} {i}: the proposal has {n}")))? = false;
        }
        Ok(keep)
    };
    let mut moved = proposal.seams.clone();
    for m in choices.move_seams.iter().flatten() {
        if !(m.by.is_finite() && m.by.abs() <= 1.0e5) {
            return Err(bad(cmd, "`move_seams` must move by a length in mm"));
        }
        let seam =
            moved.get_mut(m.seam).ok_or_else(|| bad(cmd, format!("there is no seam {}: the proposal has {}", m.seam, proposal.seams.len())))?;
        let shift = if seam.upright { Vec2::new(m.by, 0.0) } else { Vec2::new(0.0, m.by) };
        seam.from += shift;
        seam.to += shift;
    }
    Ok(PanelKeep {
        cuts: marks(&choices.drop_cutouts, proposal.cuts.len(), "cut-out")?,
        seams: marks(&choices.drop_seams, proposal.seams.len(), "seam")?,
        holes: marks(&choices.drop_holes, proposal.holes.len(), "hole")?,
        moved,
    })
}

/// The panel laid out as `keep` says.
pub fn panel_laid(pp: &PanelPrepared, keep: &PanelKeep) -> panel::Laid {
    panel::lay(&pp.proposal, &keep.cuts, &keep.seams, &keep.holes, Some(&keep.moved))
}

fn edges_out(pp: &PanelPrepared) -> PanelEdgesOut {
    let e = pp.proposal.edges;
    PanelEdgesOut { left: e.left - pp.base.x, right: e.right - pp.base.x, bottom: e.bottom - pp.base.y, top: e.top - pp.base.y }
}

const SIDE_NAMES: [&str; 4] = ["bottom", "right", "top", "left"];

fn panel_proposals(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.panel_proposals";
    let mut asked = p.clone();
    if let Some(o) = asked.as_object_mut() {
        o.insert("outline".into(), json!("scan"));
        o.entry("panel").or_insert(json!({}));
    }
    let pp = prepare_panel(s, &asked, cmd)?;
    let keep = panel_keep(&asked, &pp.proposal, cmd)?;
    let laid = panel_laid(&pp, &keep);
    let frame = &pp.prepared.made.frame;
    let normal = frame.x.cross(frame.y);
    let cutouts = pp
        .proposal
        .cuts
        .iter()
        .map(|c| {
            let (lo, hi) = c.ring.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
                (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
            });
            PanelCutoutProposal {
                notch: c.notch,
                edges: c.edges.iter().filter_map(|&k| SIDE_NAMES.get(k)).map(|n| (*n).to_owned()).collect(),
                depth: c.depth,
                area: c.area,
                size: [hi.x - lo.x, hi.y - lo.y],
                center: vec3_of(pp.at((lo + hi) / 2.0)),
                ring: c.ring.iter().map(|q| pp.wall(*q)).collect(),
            }
        })
        .collect();
    let seams = keep
        .moved
        .iter()
        .map(|sm| PanelSeamProposal {
            upright: sm.upright,
            between_rows: sm.between_rows,
            from: pp.wall(sm.from),
            to: pp.wall(sm.to),
            from_point: vec3_of(pp.at(sm.from)),
            to_point: vec3_of(pp.at(sm.to)),
        })
        .collect();
    let holes = pp
        .proposal
        .holes
        .iter()
        .map(|h| PanelHoleProposal { at: pp.wall(h.at), center: vec3_of(pp.at(h.at)), diameter: h.diameter, behind: h.behind })
        .collect();
    let reply = ScanPanelProposalsResult {
        body: pp.prepared.body.clone(),
        origin: vec3_of(pp.at(pp.base)),
        x_axis: vec3_of(frame.x),
        y_axis: vec3_of(frame.y),
        normal: vec3_of(normal),
        edges: edges_out(&pp),
        cutouts,
        seams,
        holes,
        holes_found: pp.prepared.holes.len(),
        flattened: pp.proposal.flattened,
        flattened_depth: pp.proposal.flattened_depth,
        sheet: pp.proposal.sheet,
        pieces: panel::piece_sizes(&pp.proposal, &keep.seams, Some(&keep.moved)),
        over: pp.proposal.over,
        outline: laid.outline.iter().map(|q| pp.wall(*q)).collect(),
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn flat_template(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.flat_template";
    if s.active_sketch.is_some() {
        return Err(bad(cmd, "finish the sketch being edited first"));
    }
    let mut warnings = Vec::new();
    // A straight panel: its outline and cut-outs in place of the outline as found, and its seams and
    // holes to draw.
    let (prepared, laid) = if panel_choices(p, cmd)?.is_some() {
        let pp = prepare_panel(s, p, cmd)?;
        let keep = panel_keep(p, &pp.proposal, cmd)?;
        let laid = panel_laid(&pp, &keep);
        let made_panel = PanelMade {
            edges: edges_out(&pp),
            seams: laid.seams.len(),
            holes: laid.holes.len(),
            pieces: panel::piece_sizes(&pp.proposal, &keep.seams, Some(&keep.moved)),
            over: pp.proposal.over,
        };
        let mut prepared = pp.prepared;
        let made = &mut prepared.made;
        made.outline = laid.outline.clone();
        made.area = solidcraft_scan::profile::area(&made.outline).abs() / 2.0;
        made.cutouts = laid
            .inside
            .iter()
            .map(|ring| template::Cutout {
                area: solidcraft_scan::profile::area(ring).abs() / 2.0,
                ring: ring.clone(),
                side: Side::Out,
                height: 0.0,
                at_scan_edge: false,
                crosses: false,
                clipped: false,
            })
            .collect();
        made.cut_area = made.cutouts.iter().map(|c| c.area).sum();
        if bool_(p, "make_face").unwrap_or(false) {
            warnings.push("`make_face` is not made with `panel`: the seams split the panel's region".to_owned());
        }
        (prepared, Some((laid, made_panel)))
    } else {
        (prepare(s, p, cmd)?, None)
    };
    let Prepared { body, made, point, tolerance, fitted, kept, under, name, show_scan_edge, step, moved: corner_moved, .. } = prepared;

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
        // A panel's seams (lines where the sheets meet) and the frame's holes to drill.
        if let Some((laid, _)) = &laid {
            let seams: Vec<(Outline, bool)> =
                laid.seams.iter().flatten().map(|(a, b)| (Outline::Chain(vec![Piece::Line { from: *a + shift, to: *b + shift }]), false)).collect();
            super::section::draw(sk, &seams, &mut added)?;
            let holes: Vec<(Outline, bool)> =
                laid.holes.iter().map(|(c, d)| (Outline::Circle { center: *c + shift, radius: d / 2.0 }, true)).collect();
            super::section::draw(sk, &holes, &mut added)?;
        }
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
    let face = if bool_(p, "make_face").unwrap_or(false) && laid.is_none() {
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
        face_kind: face.as_ref().map(|(_, kind)| (*kind).to_owned()),
        face: face.map(|(name, _)| name),
        edge_lengths,
        step,
        moved: corner_moved,
        panel: laid.map(|(_, made)| made),
        param_warnings: warnings,
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
