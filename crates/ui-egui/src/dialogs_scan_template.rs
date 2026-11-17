//! Flat Template (`scan.flat_template`): click the corners of the template on the wall's scan, in
//! order round it. Only those points are used; the template is laid flat on the wall's plane
//! (fitted to the scan, what stands off it left out), with a cut-out for each part standing off the
//! plane, and drawn over the view before OK sketches it: the outline, the cut-outs, and the scan's
//! own edge on the same plane to check it against.
//!
//! With the outline set to Whole Scan nothing is clicked: the whole wall's outline is found from the
//! scan, on the face it rests on, each part standing out of it a notch or a cut-out.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::{Vec2, Vec3};
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::template::panel::{Seen, Shape};
use solidcraft_engine::cmd::scan::template::{PanelKeep, PanelPrepared, panel_laid, prepare, prepare_panel};
use solidcraft_engine::geom::Aabb3;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::scan_job;
use crate::selection::{FACES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The Flat Template dialog's values (the clicks on the scan are its selection input).
#[derive(Clone, Debug)]
pub struct FlatTemplate {
    /// Where the outline comes from: 0 the points clicked, 1 the whole scan.
    pub outline: usize,
    /// With the whole scan: tears in its edge narrower than this are bridged (mm).
    pub gap: f64,
    /// With the whole scan: notches, cut-outs and slivers narrower than this are smoothed away (mm).
    pub smooth: f64,
    /// With the whole scan: the grid the template is squared and rounded to (mm; 0 as found).
    pub round: f64,
    /// With the whole scan: fingers of the template narrower than this between parts standing
    /// out are cut off (mm; 0 keeps them).
    pub finger: f64,
    /// Which parts are cut out: both sides, out, in.
    pub side: usize,
    /// How far off the plane a part must stand; 0 for what the wall's noise says.
    pub tolerance: f64,
    pub edge: bool,
    /// The scan is coloured by how far it stands off the template's plane (on by default).
    pub colours: bool,
    /// A tolerance tried here instead of the scan's own, mm (0: the scan's, see `scan.tolerance`).
    pub scan_tolerance: f64,
    /// With the whole scan: a straight panel cut from sheets, each thing proposed for it looked
    /// through and kept or dropped.
    pub panel: bool,
    /// The sheet, long × short (mm).
    pub sheet: [f64; 2],
    /// Quarter turns of the panel in the wall's plane: which edge is the floor.
    pub turn: u8,
    /// The straight edges as moved (left, right, bottom, top, mm from the outline's corner); none:
    /// as proposed.
    pub edges: Option<[f64; 4]>,
    /// The gap at edges, cut-outs and seams, and how often the edges are fastened (mm).
    pub clearance: f64,
    pub spacing: f64,
    /// Seams moved square to themselves (mm), by their number; worked out again when changed.
    pub seam_moves: BTreeMap<usize, f64>,
    /// Another scan to check the holes against (the wall opposite, or another pass of this one).
    pub confirm_with: Option<String>,
    /// What the panel is squared to: 0 the frame's holes when they say the wall is turned, 1 the
    /// frame's holes always, 2 the outline's own sides.
    pub square_to: usize,
    /// With the whole scan: which scan, when the design has more than one (the one selected when the
    /// dialog opened, or chosen in it).
    pub scan: Option<String>,
    review: Review,
    /// Where the view is to frame next (done each frame by [`tick`]).
    focus: Option<Aabb3>,
    /// Export Drawings was pressed: [`tick`] asks where and writes them.
    export_asked: bool,
    /// The scan's tolerance as changed here, kept by OK.
    keep_tolerance: Option<(String, f64)>,
    /// The template being worked out beside the window (a large scan), for the values it was begun for.
    pending: Option<Arc<scan_job::Job<String, Computed>>>,
    shown: Option<Box<Shown>>,
}

impl Default for FlatTemplate {
    fn default() -> Self {
        FlatTemplate {
            outline: 0,
            gap: solidcraft_engine::cmd::scan::template::GAP,
            smooth: solidcraft_engine::cmd::scan::template::SMOOTH,
            round: solidcraft_engine::cmd::scan::template::ROUND,
            finger: solidcraft_engine::cmd::scan::template::FINGER,
            side: 0,
            tolerance: 0.0,
            edge: true,
            colours: true,
            scan_tolerance: 0.0,
            panel: false,
            sheet: solidcraft_engine::cmd::scan::template::panel::SHEET,
            turn: 0,
            edges: None,
            clearance: solidcraft_engine::cmd::scan::template::panel::CLEARANCE,
            spacing: solidcraft_engine::cmd::scan::template::panel::SPACING,
            seam_moves: BTreeMap::new(),
            confirm_with: None,
            square_to: 0,
            scan: None,
            review: Review::default(),
            focus: None,
            export_asked: false,
            keep_tolerance: None,
            pending: None,
            shown: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<Marked, String>,
    under: Option<Under>,
}

/// The scan over the template, each point by how far it stands off the template's plane (along the
/// side `Out` goes), and the coloured dots made for a tolerance and colours.
#[derive(Clone, Debug)]
struct Under {
    scan: String,
    at: Vec<Vec3>,
    values: Vec<f32>,
    /// Whether the point is inside a cut-out: a part standing off the wall by design, which does not
    /// count against how flat the wall is.
    cut: Vec<bool>,
    /// The scan's triangles over the template, each with how far its middle stands off the plane:
    /// the colouring painted over the scan in wireframe. Empty past [`PAINT_LIMIT`]: then the
    /// points are dots.
    paint: Vec<([Vec3; 3], f32)>,
    /// Each of the scan's points by how far it stands off the plane, not a number off the
    /// template: the colouring painted on the scan's own triangles in a shaded view
    /// ([`crate::scan_paint`]), every point, however large the scan.
    vertices: Arc<Vec<f32>>,
    dots: Option<(u64, crate::scan_deviation::Colours, u64, crate::dialogs_scan_surface::Shaded)>,
}

/// The colouring's id among the clouds the viewport draws, and the most points it shows.
const SHADE_CLOUD_ID: u64 = u64::MAX - 12;
const SHADE_POINTS: usize = 40_000;
/// The most of the scan's triangles the colouring paints (more and its points are dots).
const PAINT_LIMIT: usize = crate::scan_dots::PAINT_MOST;

/// The template as it will lie, in space.
#[derive(Clone, Debug)]
struct Marked {
    outline: Vec<Vec3>,
    cutouts: Vec<Vec<Vec3>>,
    edge: Vec<Vec<Vec3>>,
    /// The wall's plane as a ghost over the outline, a little larger than it.
    ghost: [Vec3; 4],
    /// Each side of the outline: its middle and its length (mm), the dimension between two
    /// neighbouring points picked.
    sides: Vec<(Vec3, f64)>,
    area: f64,
    cut_area: f64,
    tolerance: f64,
    /// With the whole scan: the grid's cell, how true the outline is (mm).
    step: Option<f64>,
    /// With the whole scan rounded: the most a corner moved (mm).
    moved: Option<f64>,
    fitted: bool,
    kept: usize,
    under: usize,
    /// With a straight panel: what is proposed for it.
    panel: Option<Arc<PanelPrepared>>,
}

/// One thing proposed for a straight panel, as looked through: an edge (0 bottom, 1 right, 2 top,
/// 3 left), a cut-out, a seam, a hole or a fixing point, by its number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Item {
    Edge(u8),
    Cut(usize),
    Seam(usize),
    Hole(usize),
    Fixing(usize),
    Insert(usize),
}

/// Looking through a straight panel's proposal: where, what was dropped or moved, what was seen.
#[derive(Clone, Debug, Default)]
struct Review {
    /// The proposal this is about (a new one starts the look-through again).
    key: String,
    at: usize,
    dropped: BTreeSet<Item>,
    /// Cut-outs cut another way than proposed.
    shapes: BTreeMap<usize, Shape>,
    seen: BTreeSet<Item>,
}

/// The look-through a shown key is for: the same when only a seam was moved (the proposal is
/// worked out again round it, but what was looked at stays).
fn review_key(key: &str) -> String {
    let Some((given, revision)) = key.rsplit_once('|') else { return key.to_owned() };
    let Ok(mut v) = serde_json::from_str::<Value>(given) else { return key.to_owned() };
    if let Some(panel) = v.get_mut("panel").and_then(Value::as_object_mut) {
        panel.remove("move_seams");
    }
    format!("{v}|{revision}")
}

const SIDES: [&str; 3] = ["both", "out", "in"];

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.flat_template").then(|| {
        // The scan selected as the dialog opens: the one a whole outline is found from.
        let state = app.session.model.state();
        let scan = app.session.selection.iter().find_map(|x| match x {
            Sel::Body { name } | Sel::Face { body: name, .. } if state.body(name).is_some_and(|b| b.body.is_mesh()) => Some(name.clone()),
            _ => None,
        });
        (Kind::ScanFlatTemplate(FlatTemplate { scan, ..FlatTemplate::default() }), vec![SelInput::new("Outline Points", FACES, true).places()])
    })
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// What `scan.flat_template` is sent: the clicks in order, once there are three, or the whole scan.
fn params(k: &FlatTemplate, inputs: &[SelInput]) -> Option<Value> {
    if k.outline == 1 {
        let mut p = json!({
            "outline": "scan",
            "gap": k.gap,
            "smooth": k.smooth,
            "round": k.round,
            "finger": k.finger,
            "side": SIDES[k.side.min(SIDES.len() - 1)],
            "show_scan_edge": k.edge,
        });
        if let Some(Sel::Face { body, .. }) = items(inputs, 0).first() {
            p["body"] = json!(body);
        } else if let Some(scan) = &k.scan {
            p["body"] = json!(scan);
        }
        // With the whole scan, a click on a raised part covers it rather than cutting round it.
        let picked = clicks(inputs);
        if !picked.is_empty() {
            p["cover"] = json!(picked.iter().map(|q| [q.x, q.y, q.z]).collect::<Vec<_>>());
        }
        if k.tolerance > 0.0 {
            p["tolerance"] = json!(k.tolerance);
        }
        if k.panel {
            let mut panel = json!({"sheet": k.sheet, "turn": k.turn % 4, "clearance": k.clearance, "spacing": k.spacing});
            if let Some(other) = &k.confirm_with {
                panel["confirm_with"] = json!(other);
            }
            if let Some(to) = [None, Some("holes"), Some("outline")].get(k.square_to).copied().flatten() {
                panel["square_to"] = json!(to);
            }
            let moves: Vec<Value> = k.seam_moves.iter().filter(|(_, by)| **by != 0.0).map(|(seam, by)| json!({"seam": seam, "by": by})).collect();
            if !moves.is_empty() {
                panel["move_seams"] = json!(moves);
            }
            if let Some([left, right, bottom, top]) = k.edges {
                panel["edges"] = json!({"left": left, "right": right, "bottom": bottom, "top": top});
            }
            p["panel"] = panel;
        }
        return Some(p);
    }
    let clicks: Vec<(&str, Vec3)> = items(inputs, 0)
        .iter()
        .filter_map(|s| match s {
            Sel::Face { body, point, .. } => Some((body.as_str(), *point)),
            _ => None,
        })
        .collect();
    let (body, _) = *clicks.first()?;
    if clicks.len() < 3 {
        return None;
    }
    let mut p = json!({
        "body": body,
        "outline": clicks.iter().map(|(_, q)| json!([q.x, q.y, q.z])).collect::<Vec<_>>(),
        "side": SIDES[k.side.min(SIDES.len() - 1)],
        "show_scan_edge": k.edge,
    });
    if k.tolerance > 0.0 {
        p["tolerance"] = json!(k.tolerance);
    }
    Some(p)
}

/// Work the template out again when the clicks, the values or the design have changed.
fn look(app: &SolveApp, k: &mut FlatTemplate, inputs: &[SelInput]) {
    let Some(p) = params(k, inputs) else {
        k.shown = None;
        k.pending = None;
        return;
    };
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        k.pending = None;
        return;
    }
    // On a large scan the template is worked out beside the window and what was drawn stays until it arrives.
    if scan_job::beside_window(app) {
        if let Some(job) = k.pending.clone() {
            match job.poll() {
                scan_job::Poll::Working if job.key == key => return,
                scan_job::Poll::Done((result, under)) if job.key == key => {
                    k.pending = None;
                    k.shown = Some(Box::new(Shown { key, result, under }));
                    return;
                }
                _ => k.pending = None,
            }
        }
        let given = p.clone();
        if let Some(job) = scan_job::spawn(app, key.clone(), move |s| compute(s, &given)) {
            k.pending = Some(Arc::new(job));
            return;
        }
    }
    let (result, under) = compute(&app.session, &p);
    k.shown = Some(Box::new(Shown { key, result, under }));
}

/// The template `p` asks for and the scan over it: all that looks over the scan.
type Computed = (Result<Marked, String>, Option<Under>);

fn compute(s: &solidcraft_engine::Session, p: &Value) -> Computed {
    let mut under = None;
    let panel = p.get("panel").is_some();
    let prepared = if panel {
        prepare_panel(s, p, "scan.flat_template").map(|pp| (pp.prepared.clone(), Some(Arc::new(pp))))
    } else {
        prepare(s, p, "scan.flat_template").map(|m| (m, None))
    };
    let result = prepared.map_err(|e| e.to_string()).map(|(m, panel)| {
        under = under_of(s, &m);
        let t = &m.made;
        let place = |ring: &[solidcore_record::Vec2]| -> Vec<Vec3> { ring.iter().map(|q| t.at(*q)).collect() };
        let (lo, hi) = t.outline.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
            (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
        });
        let grow = 0.15 * (hi.x - lo.x).max(hi.y - lo.y);
        let (lo, hi) = (Vec2::new(lo.x - grow, lo.y - grow), Vec2::new(hi.x + grow, hi.y + grow));
        // A found outline has hundreds of sides: no lengths over it.
        let sides = if m.step.is_some() {
            Vec::new()
        } else {
            t.outline.iter().zip(t.outline.iter().cycle().skip(1)).map(|(a, b)| (t.at((*a + *b) / 2.0), a.dist(*b))).collect()
        };
        Marked {
            ghost: [t.at(lo), t.at(Vec2::new(hi.x, lo.y)), t.at(hi), t.at(Vec2::new(lo.x, hi.y))],
            sides,
            outline: place(&t.outline),
            cutouts: t.cutouts.iter().map(|c| place(&c.ring)).collect(),
            edge: if m.show_scan_edge { t.scan_edge.iter().map(|l| place(l)).collect() } else { Vec::new() },
            area: t.area,
            cut_area: t.cut_area,
            tolerance: m.tolerance,
            step: m.step,
            moved: m.moved,
            fitted: m.fitted,
            kept: m.kept,
            under: m.under,
            panel,
        }
    });
    (result, under)
}

/// Whether `p` is inside the ring (even-odd), both in the template's coordinates.
fn inside(ring: &[Vec2], p: Vec2) -> bool {
    let mut on = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            on = !on;
        }
    }
    on
}

/// The scan's points over the template's outline and how far each stands off its plane: thinned
/// evenly to [`SHADE_POINTS`].
fn under_of(s: &solidcraft_engine::Session, m: &solidcraft_engine::cmd::scan::template::Prepared) -> Option<Under> {
    let state = s.model.state();
    let (positions, triangles) = state.body(&m.body)?.body.triangle_mesh()?;
    let t = &m.made;
    let normal = t.frame.x.cross(t.frame.y).normalized()?;
    let step = (positions.len() / SHADE_POINTS).max(1);
    let local = |p: Vec3| {
        let d = p - t.frame.origin;
        Vec2::new(d.dot(t.frame.x), d.dot(t.frame.y))
    };
    let mut under =
        Under { scan: m.body.clone(), at: Vec::new(), values: Vec::new(), cut: Vec::new(), paint: Vec::new(), vertices: Arc::default(), dots: None };
    for p in positions.iter().step_by(step) {
        let q = local(*p);
        if !inside(&t.outline, q) {
            continue;
        }
        under.at.push(*p);
        under.values.push(((*p - t.frame.origin).dot(normal)) as f32);
        under.cut.push(t.cutouts.iter().any(|c| inside(&c.ring, q)));
    }
    let off = |p: Vec3| ((p - t.frame.origin).dot(normal)) as f32;
    under.vertices = Arc::new(over_outline(positions, &t.outline, local, off));
    under.paint = painted(positions, triangles, &t.outline, local, off);
    if under.paint.is_empty() {
        under.paint = gridded(positions, triangles, &t.outline, local, off, |q| t.at(q), normal);
    }
    (!under.at.is_empty()).then_some(under)
}

/// Each of the scan's `positions` with `value` where it lies inside the template's `outline`
/// (`local` puts a point in its coordinates), not a number elsewhere. The outline's box is tried
/// first, so the outline is walked only for points near it.
fn over_outline(positions: &[Vec3], outline: &[Vec2], local: impl Fn(Vec3) -> Vec2, value: impl Fn(Vec3) -> f32) -> Vec<f32> {
    let Some(first) = outline.first() else { return vec![f32::NAN; positions.len()] };
    let (lo, hi) =
        outline.iter().fold((*first, *first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
    positions
        .iter()
        .map(|p| {
            let q = local(*p);
            let near = q.x >= lo.x && q.y >= lo.y && q.x <= hi.x && q.y <= hi.y;
            if near && inside(outline, q) { value(*p) } else { f32::NAN }
        })
        .collect()
}

/// The scan's triangles whose middles lie inside the template's `outline` (`local` puts a point in
/// its coordinates), each with `value` at its middle; none past [`PAINT_LIMIT`]. The outline's box
/// is tried first, so a found outline of hundreds of sides is walked only for triangles near it.
fn painted(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    outline: &[Vec2],
    local: impl Fn(Vec3) -> Vec2,
    value: impl Fn(Vec3) -> f32,
) -> Vec<([Vec3; 3], f32)> {
    let Some(first) = outline.first() else { return Vec::new() };
    let (lo, hi) =
        outline.iter().fold((*first, *first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
    let mut out = Vec::new();
    for tri in triangles {
        let corner = |i: u32| positions.get(i as usize).copied();
        let (Some(a), Some(b), Some(c)) = (corner(tri[0]), corner(tri[1]), corner(tri[2])) else { continue };
        let middle = (a + b + c) / 3.0;
        let q = local(middle);
        if q.x < lo.x || q.y < lo.y || q.x > hi.x || q.y > hi.y || !inside(outline, q) {
            continue;
        }
        if out.len() == PAINT_LIMIT {
            return Vec::new();
        }
        out.push(([a, b, c], value(middle)));
    }
    out
}

/// The scan over the template as a grid of cells for a scan with too many triangles to paint (a
/// whole wall of millions): each cell at the scan's mean height in it, its corners at the mean of
/// the cells round them so the grid lies on the scan as one sheet, coloured by its own mean. As
/// many cells as fit [`PAINT_LIMIT`] triangles; a cell with no scan in it, or outside the
/// outline, is left out. Its triangles face the way the scan's do.
fn gridded(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    outline: &[Vec2],
    local: impl Fn(Vec3) -> Vec2,
    offset: impl Fn(Vec3) -> f32,
    at: impl Fn(Vec2) -> Vec3,
    normal: Vec3,
) -> Vec<([Vec3; 3], f32)> {
    let Some(first) = outline.first() else { return Vec::new() };
    let (lo, hi) =
        outline.iter().fold((*first, *first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
    let (w, h) = (hi.x - lo.x, hi.y - lo.y);
    let size = (w * h / (PAINT_LIMIT / 2) as f64).sqrt();
    if !(size.is_finite() && size > 0.0) {
        return Vec::new();
    }
    let (nx, ny) = ((w / size).ceil().max(1.0) as usize, (h / size).ceil().max(1.0) as usize);
    let mut sums = vec![(0.0f64, 0u32); nx * ny];
    let mut facing = 0.0;
    for tri in triangles {
        let corner = |i: u32| positions.get(i as usize).copied();
        let (Some(a), Some(b), Some(c)) = (corner(tri[0]), corner(tri[1]), corner(tri[2])) else { continue };
        let middle = (a + b + c) / 3.0;
        let q = local(middle);
        let (i, j) = (((q.x - lo.x) / size).floor(), ((q.y - lo.y) / size).floor());
        if !(i >= 0.0 && j >= 0.0 && (i as usize) < nx && (j as usize) < ny) {
            continue;
        }
        let cell = &mut sums[j as usize * nx + i as usize];
        cell.0 += f64::from(offset(middle));
        cell.1 += 1;
        facing += (b - a).cross(c - a).dot(normal);
    }
    let mean = |i: usize, j: usize| sums.get(j * nx + i).filter(|c| c.1 > 0).map(|c| c.0 / f64::from(c.1));
    // A grid corner's height: the mean of the cells round it that have scan.
    let corner = |i: usize, j: usize| {
        let round: Vec<f64> = [(i.wrapping_sub(1), j.wrapping_sub(1)), (i, j.wrapping_sub(1)), (i.wrapping_sub(1), j), (i, j)]
            .into_iter()
            .filter(|(a, b)| *a < nx && *b < ny)
            .filter_map(|(a, b)| mean(a, b))
            .collect();
        (!round.is_empty()).then(|| round.iter().sum::<f64>() / round.len() as f64)
    };
    let place = |i: usize, j: usize, height: f64| at(Vec2::new(lo.x + i as f64 * size, lo.y + j as f64 * size)) + normal * height;
    let mut out = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let Some(value) = mean(i, j) else { continue };
            let middle = Vec2::new(lo.x + (i as f64 + 0.5) * size, lo.y + (j as f64 + 0.5) * size);
            if !inside(outline, middle) {
                continue;
            }
            let height = |a: usize, b: usize| corner(a, b).unwrap_or(value);
            let [p00, p10, p11, p01] = [
                place(i, j, height(i, j)),
                place(i + 1, j, height(i + 1, j)),
                place(i + 1, j + 1, height(i + 1, j + 1)),
                place(i, j + 1, height(i, j + 1)),
            ];
            let v = value as f32;
            // Wound so each triangle faces the way the scan does.
            if facing >= 0.0 {
                out.extend([([p00, p10, p11], v), ([p00, p11, p01], v)]);
            } else {
                out.extend([([p00, p11, p10], v), ([p00, p01, p11], v)]);
            }
        }
    }
    out
}

/// The scan's colouring made again when the tolerance or the colours have changed: its triangles
/// painted, or its points as dots where too many lie over the template.
fn colour(app: &SolveApp, k: &mut FlatTemplate) {
    let colours = crate::scan_deviation::colours();
    let Some(under) = k.shown.as_mut().and_then(|s| s.under.as_mut()) else { return };
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.scan_tolerance);
    if under.dots.as_ref().is_some_and(|(t, c, _, _)| *t == tolerance.to_bits() && *c == colours) {
        return;
    }
    let rgb = |v: f32| crate::dialogs_scan_surface::solidcraft_scan_scale(v, tolerance).map(|s| crate::scan_deviation::colour(colours, s));
    let shaded = if under.paint.is_empty() {
        let mut out = Vec::with_capacity(under.at.len() * crate::scan_dots::DOT_SIZE);
        for (at, v) in under.at.iter().zip(&under.values) {
            let Some(rgb) = rgb(*v) else { continue };
            crate::scan_dots::cloud_dot(&mut out, &crate::scan_dots::Dot { at: at.to_f32(), value: *v, rgb });
        }
        crate::dialogs_scan_surface::Shaded { dots: Arc::new(out), triangles: crate::scan_dots::none(), scale: None }
    } else {
        let mut out = Vec::with_capacity(under.paint.len() * crate::scan_dots::DOT_SIZE);
        for (corners, v) in &under.paint {
            let Some(rgb) = rgb(*v) else { continue };
            crate::scan_dots::cloud_painted(&mut out, *corners, *v, rgb);
        }
        crate::dialogs_scan_surface::Shaded { dots: crate::scan_dots::none(), triangles: Arc::new(out), scale: None }
    };
    under.dots = Some((tolerance.to_bits(), colours, crate::dialogs_scan_surface::next_shade_version(), shaded));
}

/// How flat the wall is under the template, and the tolerance: the points outside the cut-outs,
/// for the viewport's key and the dialog's line.
pub(crate) fn fidelity(app: &SolveApp, k: &FlatTemplate) -> Option<(crate::scan_fidelity::Stats, f64)> {
    if !k.colours {
        return None;
    }
    let under = k.shown.as_ref()?.under.as_ref()?;
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.scan_tolerance);
    let wall: Vec<f32> = under.values.iter().zip(&under.cut).filter(|(_, c)| !**c).map(|(v, _)| *v).collect();
    Some((crate::scan_fidelity::stats(&wall, tolerance)?, tolerance))
}

thread_local! {
    /// The colouring last put on the scan's own triangles: the points' values it was made from
    /// (held, so their address stays theirs), the tolerance and colours, the painting's key and
    /// where each point sits on the scale.
    #[allow(clippy::type_complexity)]
    static PAINTED: std::cell::RefCell<Option<(Arc<Vec<f32>>, u64, crate::scan_deviation::Colours, u64, Arc<Vec<f32>>)>> =
        const { std::cell::RefCell::new(None) };
}

/// The colouring painted on the scan's own triangles ([`crate::scan_paint`]) while the dialog
/// shows it: each of the scan's points over the template by how far it stands off the plane.
pub(crate) fn painting(app: &SolveApp) -> Option<crate::scan_paint::Painting> {
    let Some(Kind::ScanFlatTemplate(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    if !k.colours {
        return None;
    }
    let under = k.shown.as_ref()?.under.as_ref()?;
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.scan_tolerance);
    let colours = crate::scan_deviation::colours();
    let (key, scale) = PAINTED.with(|p| {
        let mut p = p.borrow_mut();
        if let Some((v, t, c, key, scale)) = p.as_ref()
            && Arc::ptr_eq(v, &under.vertices)
            && *t == tolerance.to_bits()
            && *c == colours
        {
            return (*key, scale.clone());
        }
        let scale: Arc<Vec<f32>> = Arc::new(
            under
                .vertices
                .iter()
                .map(|v| {
                    if v.is_nan() {
                        f32::NAN
                    } else {
                        crate::dialogs_scan_surface::solidcraft_scan_scale(*v, tolerance).map_or(f32::NAN, |s| s as f32)
                    }
                })
                .collect(),
        );
        let key = crate::dialogs_scan_surface::next_shade_version();
        *p = Some((under.vertices.clone(), tolerance.to_bits(), colours, key, scale.clone()));
        (key, scale)
    });
    Some(crate::scan_paint::Painting { scan: under.scan.clone(), key, scale, colours })
}

/// The colouring of the scan over the template, as a cloud for the GPU viewport: in wireframe
/// only, a shaded view paints the scan's own triangles ([`painting`]).
pub(crate) fn gpu_shade(app: &SolveApp) -> Option<solidcraft_viewport_gpu::view::GpuCloud> {
    let Some(Kind::ScanFlatTemplate(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    if !k.colours || crate::scan_paint::on_scan(app) {
        return None;
    }
    let (_, _, version, shaded) = k.shown.as_ref()?.under.as_ref()?.dots.as_ref()?;
    Some(crate::dialogs_scan_surface::shade_cloud(SHADE_CLOUD_ID, *version, shaded))
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FlatTemplate, inputs: &[SelInput]) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let was = k.outline;
    choice_row(
        ui,
        tr("ui.scan_template_outline"),
        "scan_template_outline",
        &mut k.outline,
        &[tr("ui.scan_template_picked"), tr("ui.scan_template_whole")],
    );
    if k.outline != was && k.outline == 1 {
        // A whole wall: cut what stands out into the room, and leave its ragged edge undrawn.
        k.side = 1;
        k.edge = false;
    }
    if k.outline == 1 {
        // With more than one scan in the design: which one the wall's outline is found from.
        let meshes: Vec<String> = app.session.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect();
        if meshes.len() > 1 {
            let labels: Vec<&str> = meshes.iter().map(String::as_str).collect();
            let mut pick = k.scan.as_ref().and_then(|n| meshes.iter().position(|m| m == n)).unwrap_or(0);
            choice_row(ui, tr("ui.scan_panel_scan"), "scan_panel_scan", &mut pick, &labels);
            k.scan = meshes.get(pick).cloned();
            if k.confirm_with == k.scan {
                k.confirm_with = None;
            }
        }
        form_row(ui, tr("ui.scan_template_gap"), |ui| ui_kit::length_field(ui, &mut k.gap, &units, decimals));
        k.gap = k.gap.max(0.0);
        form_row(ui, tr("ui.scan_template_smooth"), |ui| ui_kit::length_field(ui, &mut k.smooth, &units, decimals));
        k.smooth = k.smooth.max(0.0);
        form_row(ui, tr("ui.scan_template_round"), |ui| ui_kit::length_field(ui, &mut k.round, &units, decimals));
        k.round = k.round.max(0.0);
        form_row(ui, tr("ui.scan_template_finger"), |ui| ui_kit::length_field(ui, &mut k.finger, &units, decimals));
        k.finger = k.finger.max(0.0);
        let was_panel = k.panel;
        form_row(ui, tr("ui.scan_panel"), |ui| check(ui, &mut k.panel));
        if k.panel && !was_panel {
            // The panel's lines are what is looked at: the scan's colours would hide them.
            k.colours = false;
        }
        if k.panel {
            form_row(ui, tr("ui.scan_panel_sheet_long"), |ui| ui_kit::length_field(ui, &mut k.sheet[0], &units, decimals));
            form_row(ui, tr("ui.scan_panel_sheet_short"), |ui| ui_kit::length_field(ui, &mut k.sheet[1], &units, decimals));
            form_row(ui, tr("ui.scan_panel_gap"), |ui| ui_kit::length_field(ui, &mut k.clearance, &units, decimals));
            k.clearance = if k.clearance.is_finite() { k.clearance.clamp(0.0, 50.0) } else { 3.0 };
            form_row(ui, tr("ui.scan_panel_spacing"), |ui| ui_kit::length_field(ui, &mut k.spacing, &units, decimals));
            // 0 proposes no fixing points; otherwise every 50 mm at the closest.
            k.spacing = if !k.spacing.is_finite() || k.spacing <= 0.0 { 0.0 } else { k.spacing.clamp(50.0, 5000.0) };
            // Another scan to check the holes against: any other scan in the design.
            let own = k
                .scan
                .clone()
                .or_else(|| k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).and_then(|m| m.panel.as_ref()).map(|pp| pp.prepared.body.clone()));
            let scans: Vec<String> = app
                .session
                .model
                .state()
                .bodies
                .iter()
                .filter(|b| b.body.is_mesh() && own.as_deref() != Some(b.name.as_str()))
                .map(|b| b.name.clone())
                .collect();
            if !scans.is_empty() || k.confirm_with.is_some() {
                let mut labels: Vec<&str> = vec![tr("ui.scan_panel_confirm_none")];
                labels.extend(scans.iter().map(String::as_str));
                let mut pick = k.confirm_with.as_ref().and_then(|c| scans.iter().position(|s| s == c)).map_or(0, |i| i + 1);
                choice_row(ui, tr("ui.scan_panel_confirm"), "scan_panel_confirm", &mut pick, &labels);
                k.confirm_with = pick.checked_sub(1).and_then(|i| scans.get(i)).cloned();
            }
            let was_square = k.square_to;
            let square = [tr("ui.scan_panel_square_auto"), tr("ui.scan_panel_square_holes"), tr("ui.scan_panel_square_outline")];
            choice_row(ui, tr("ui.scan_panel_square"), "scan_panel_square", &mut k.square_to, &square);
            if k.square_to != was_square {
                // The edges were for the panel as it lay.
                k.edges = None;
            }
            k.sheet = [k.sheet[0].clamp(100.0, 100_000.0), k.sheet[1].clamp(100.0, 100_000.0)];
            form_row(ui, tr("ui.scan_panel_floor"), |ui| {
                if ui_kit::button(ui, tr("ui.scan_panel_turn"), ui_kit::Button::Small).clicked() {
                    k.turn = (k.turn + 1) % 4;
                    // The edges were for the panel as it lay.
                    k.edges = None;
                }
            });
            ui_kit::text(ui, tr("ui.scan_panel_floor_hint"), ui_kit::Text::Hint("scan_panel_floor_hint"));
        }
    }
    let sides = [tr("ui.scan_template_both"), tr("ui.scan_template_out"), tr("ui.scan_template_in")];
    choice_row(ui, tr("ui.scan_template_side"), "scan_template_side", &mut k.side, &sides);
    let ways = [tr("ui.scan_fit_detail_auto"), tr("ui.scan_fit_detail_set")];
    crate::dialogs_scan_surface::own_or_set(
        app,
        ui,
        "scan_template_standoff",
        tr("ui.scan_template_standoff"),
        ways,
        tr("ui.scan_template_stands_off"),
        &mut k.tolerance,
        1.0,
    );
    form_row(ui, tr("ui.scan_template_edge"), |ui| check(ui, &mut k.edge));
    look(app, k, inputs);
    if k.shown.as_ref().is_some_and(|s| s.under.is_some()) {
        crate::dialogs_scan_surface::colours_row(ui, &mut k.colours);
        let scan = k.shown.as_ref().and_then(|s| s.under.as_ref()).map(|u| u.scan.clone());
        k.keep_tolerance = crate::scan_fidelity::row(app, ui, scan.as_deref(), &mut k.scan_tolerance).keep;
    }
    colour(app, k);
    let panel = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).and_then(|m| m.panel.clone());
    if k.panel
        && k.outline == 1
        && let Some(pp) = panel
    {
        review_rows(app, ui, k, &pp);
    }
    if k.pending.is_some() {
        scan_job::refining_note(ui, "scan_template_refining");
    }
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let line = solidcraft_ui_text::format(
                "ui.scan_template_found",
                &[
                    ("cutouts", &m.cutouts.len().to_string()),
                    ("area", &format!("{:.0}", m.area)),
                    ("cut", &format!("{:.0}", m.cut_area)),
                    ("tolerance", &format!("{:.1}", m.tolerance)),
                ],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_template_found"));
            if let Some(step) = m.step {
                let how = solidcraft_ui_text::format("ui.scan_template_whole_hint", &[("step", &format!("{step:.1}"))]);
                ui_kit::text(ui, &how, ui_kit::Text::Note("scan_template_whole_hint"));
                ui_kit::text(ui, tr("ui.scan_template_cover_hint"), ui_kit::Text::Hint("scan_template_cover_hint"));
                if let Some(moved) = m.moved {
                    let tidied = solidcraft_ui_text::format(
                        "ui.scan_template_tidied",
                        &[("round", &format!("{:.1}", k.round)), ("moved", &format!("{moved:.1}"))],
                    );
                    ui_kit::text(ui, &tidied, ui_kit::Text::Note("scan_template_tidied"));
                }
            } else {
                let sides = m.sides.iter().map(|(_, l)| format!("{l:.1}")).collect::<Vec<_>>().join(" · ");
                let dims = solidcraft_ui_text::format("ui.scan_template_sides", &[("sides", &sides)]);
                ui_kit::text(ui, &dims, ui_kit::Text::Note("scan_template_sides"));
            }
            if let Some((stats, tolerance)) = fidelity(app, k) {
                crate::scan_fidelity::readout(app, ui, "scan_template_fidelity", &stats, tolerance, None);
            }
            if m.fitted {
                let plane = solidcraft_ui_text::format("ui.scan_template_plane", &[("kept", &m.kept.to_string()), ("under", &m.under.to_string())]);
                ui_kit::text(ui, &plane, ui_kit::Text::Note("scan_template_plane"));
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_template_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_template_hint"), ui_kit::Text::Hint("scan_template_hint"));
        }
    }
    false
}

/// What OK runs.
pub fn commands(k: &FlatTemplate, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(k, inputs).ok_or_else(|| tr("ui.scan_template_hint").to_owned())?;
    k.shown.as_ref().ok_or("still working out the template")?.result.as_ref().map_err(Clone::clone)?;
    let mut p = p;
    if let Some(panel) = p.get_mut("panel").and_then(Value::as_object_mut) {
        let numbers = |pick: fn(&Item) -> Option<usize>| k.review.dropped.iter().filter_map(pick).collect::<Vec<_>>();
        panel.insert("drop_cutouts".into(), json!(numbers(|i| if let Item::Cut(n) = i { Some(*n) } else { None })));
        panel.insert("drop_seams".into(), json!(numbers(|i| if let Item::Seam(n) = i { Some(*n) } else { None })));
        panel.insert("drop_holes".into(), json!(numbers(|i| if let Item::Hole(n) = i { Some(*n) } else { None })));
        panel.insert("drop_fixings".into(), json!(numbers(|i| if let Item::Fixing(n) = i { Some(*n) } else { None })));
        panel.insert("drop_inserts".into(), json!(numbers(|i| if let Item::Insert(n) = i { Some(*n) } else { None })));
        let shapes: Vec<Value> = k.review.shapes.iter().map(|(cutout, shape)| json!({"cutout": cutout, "shape": shape.name()})).collect();
        panel.insert("shapes".into(), json!(shapes));
    }
    let mut all = Vec::new();
    all.extend(crate::scan_fidelity::keep(k.keep_tolerance.as_ref()));
    all.push(("scan.flat_template".into(), p));
    Ok(all)
}

/// The points clicked so far, in order.
fn clicks(inputs: &[SelInput]) -> Vec<Vec3> {
    items(inputs, 0)
        .iter()
        .filter_map(|s| match s {
            Sel::Face { point, .. } => Some(*point),
            _ => None,
        })
        .collect()
}

/// A length's label at `at`.
fn label(painter: &egui::Painter, proj: &crate::viewport::Proj, at: Vec3, mm: f64) {
    let t = solidcraft_theme::Tokens::get();
    if let Some(p) = proj.to_screen(at) {
        painter.text(p, egui::Align2::CENTER_CENTER, format!("{mm:.1}"), egui::FontId::proportional(t.font_label), t.text);
    }
}

/// The wall's plane the open Flat Template lays its points on (a point on it and its normal), once
/// it is known: clicks snap onto it, so the marker shows where a corner of the template will be.
pub(crate) fn wall_plane(app: &SolveApp) -> Option<(Vec3, Vec3)> {
    let Some(Kind::ScanFlatTemplate(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let m = k.shown.as_ref()?.result.as_ref().ok()?;
    let [a, b, _, d] = m.ghost;
    let normal = (b - a).cross(d - a).normalized()?;
    Some((a, normal))
}

/// The template where it will lie, over the picture: the wall's plane as a ghost, the outline with
/// the length of each side, the cut-outs and the scan's edge. Before there is a template, the
/// points clicked so far are joined and each distance told.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(d) = app.dialog.as_ref() else { return };
    let Kind::ScanFlatTemplate(k) = &d.kind else { return };
    let tokens = solidcraft_theme::Tokens::get();
    let line = |ring: &[Vec3], looped: bool, stroke: egui::Stroke| {
        let step = (ring.len() / 20_000).max(1);
        let pts: Vec<egui::Pos2> =
            ring.iter().step_by(step).chain(looped.then(|| ring.first()).flatten()).filter_map(|q| proj.to_screen(*q)).collect();
        painter.add(egui::Shape::line(pts, stroke));
    };
    let Some(Ok(m)) = k.shown.as_ref().map(|s| &s.result) else {
        let picked = clicks(&d.inputs);
        line(&picked, false, egui::Stroke::new(1.5, tokens.accent));
        for w in picked.windows(2) {
            label(painter, proj, (w[0] + w[1]) * 0.5, w[0].dist(w[1]));
        }
        return;
    };
    let ghost: Vec<egui::Pos2> = m.ghost.iter().filter_map(|q| proj.to_screen(*q)).collect();
    if ghost.len() == 4 {
        painter.add(egui::Shape::convex_polygon(
            ghost,
            tokens.accent.gamma_multiply(0.12),
            egui::Stroke::new(1.0, tokens.accent.gamma_multiply(0.4)),
        ));
    }
    for ring in &m.edge {
        line(ring, true, egui::Stroke::new(1.0, tokens.text_dim));
    }
    if let Some(pp) = m.panel.as_ref().filter(|_| k.panel) {
        draw_panel(painter, proj, k, pp);
        return;
    }
    for ring in &m.cutouts {
        line(ring, true, egui::Stroke::new(1.5, tokens.error));
    }
    line(&m.outline, true, egui::Stroke::new(2.0, tokens.accent));
    for (at, mm) in &m.sides {
        label(painter, proj, *at, *mm);
    }
}

/// Every thing proposed, in the order they are looked through: the edges, the cut-outs, the seams,
/// the holes.
fn items_of(pp: &PanelPrepared) -> Vec<Item> {
    let p = &pp.proposal;
    (0..4u8)
        .map(Item::Edge)
        .chain((0..p.cuts.len()).map(Item::Cut))
        .chain((0..p.seams.len()).map(Item::Seam))
        .chain((0..p.holes.len()).map(Item::Hole))
        .chain((0..p.fixings.len()).map(Item::Fixing))
        .chain((0..p.inserts.len()).map(Item::Insert))
        .collect()
}

/// What is kept, as the panel is to be laid.
fn keep_of(pp: &PanelPrepared, r: &Review) -> PanelKeep {
    let p = &pp.proposal;
    PanelKeep {
        cuts: (0..p.cuts.len()).map(|i| !r.dropped.contains(&Item::Cut(i))).collect(),
        shapes: p.cuts.iter().enumerate().map(|(i, c)| r.shapes.get(&i).copied().unwrap_or(c.shape)).collect(),
        seams: (0..p.seams.len()).map(|i| !r.dropped.contains(&Item::Seam(i))).collect(),
        holes: (0..p.holes.len()).map(|i| !r.dropped.contains(&Item::Hole(i))).collect(),
        fixings: (0..p.fixings.len()).map(|i| !r.dropped.contains(&Item::Fixing(i))).collect(),
        moved: p.seams.clone(),
        inserts: (0..p.inserts.len()).map(|i| !r.dropped.contains(&Item::Insert(i))).collect(),
    }
}

/// The edge's two ends (template coordinates).
fn edge_ends(pp: &PanelPrepared, side: u8) -> (Vec2, Vec2) {
    let e = pp.proposal.edges;
    match side {
        0 => (Vec2::new(e.left, e.bottom), Vec2::new(e.right, e.bottom)),
        1 => (Vec2::new(e.right, e.bottom), Vec2::new(e.right, e.top)),
        2 => (Vec2::new(e.right, e.top), Vec2::new(e.left, e.top)),
        _ => (Vec2::new(e.left, e.top), Vec2::new(e.left, e.bottom)),
    }
}

/// The places an item is drawn through (template coordinates).
fn item_points(pp: &PanelPrepared, keep: &PanelKeep, item: Item) -> Vec<Vec2> {
    let p = &pp.proposal;
    match item {
        Item::Edge(side) => {
            let (a, b) = edge_ends(pp, side);
            vec![a, b]
        }
        Item::Cut(i) => p.cuts.get(i).map(|c| c.ring.clone()).unwrap_or_default(),
        Item::Seam(i) => keep.moved.get(i).map(|s| vec![s.from, s.to]).unwrap_or_default(),
        Item::Hole(i) => p.holes.get(i).map(|h| vec![h.at]).unwrap_or_default(),
        Item::Fixing(i) => p.fixings.get(i).map(|f| vec![f.at]).unwrap_or_default(),
        Item::Insert(i) => p.inserts.get(i).map(|o| o.ring.clone()).unwrap_or_default(),
    }
}

/// The box the view frames to look at an item: the item, and room round it to see where it is.
fn focus_of(pp: &PanelPrepared, keep: &PanelKeep, item: Item) -> Option<Aabb3> {
    let pts: Vec<Vec3> = item_points(pp, keep, item).into_iter().map(|q| pp.at(q)).collect();
    let first = *pts.first()?;
    let (lo, hi) = pts.iter().fold((first, first), |(lo, hi), q| {
        (Vec3::new(lo.x.min(q.x), lo.y.min(q.y), lo.z.min(q.z)), Vec3::new(hi.x.max(q.x), hi.y.max(q.y), hi.z.max(q.z)))
    });
    let mid = (lo + hi) * 0.5;
    let half = ((hi - lo).len() * 0.65).max(FOCUS_ROOM);
    let r = Vec3::new(half, half, half);
    Some(Aabb3 { min: mid - r, max: mid + r })
}

/// The least the view frames round an item (mm): enough to see where on the wall it is.
const FOCUS_ROOM: f64 = 250.0;

fn side_word(side: u8) -> &'static str {
    match side {
        0 => tr("ui.scan_panel_bottom"),
        1 => tr("ui.scan_panel_right"),
        2 => tr("ui.scan_panel_top"),
        _ => tr("ui.scan_panel_left"),
    }
}

/// Looking through the straight panel's proposal, one thing at a time: what it is, Keep or Drop
/// (Enter or Y, X), Back (Backspace), Keep the Rest; an edge or a seam can be moved.
fn review_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FlatTemplate, pp: &Arc<PanelPrepared>) {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let key = k.shown.as_ref().map(|s| review_key(&s.key)).unwrap_or_default();
    if k.review.key != key {
        k.review = Review { key, ..Review::default() };
    }
    let items = items_of(pp);
    let Some(last) = items.len().checked_sub(1) else { return };
    k.review.at = k.review.at.min(last);
    let item = items[k.review.at];
    let p = &pp.proposal;
    let progress = solidcraft_ui_text::format(
        "ui.scan_panel_progress",
        &[("seen", &k.review.seen.len().to_string()), ("total", &items.len().to_string()), ("dropped", &k.review.dropped.len().to_string())],
    );
    ui_kit::text(ui, &progress, ui_kit::Text::Note("scan_panel_progress"));
    let count = |n: usize| n.to_string();
    let what = match item {
        Item::Edge(side) => {
            let e = p.edges;
            let at = match side {
                0 => e.bottom - pp.base.y,
                1 => e.right - pp.base.x,
                2 => e.top - pp.base.y,
                _ => e.left - pp.base.x,
            };
            solidcraft_ui_text::format("ui.scan_panel_edge", &[("side", side_word(side)), ("at", &format!("{at:.0}"))])
        }
        Item::Cut(i) => {
            let c = &p.cuts[i];
            let (lo, hi) = c.ring.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
                (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
            });
            let args = [
                ("n", &count(i + 1)),
                ("count", &count(p.cuts.len())),
                ("w", &format!("{:.0}", hi.x - lo.x)),
                ("h", &format!("{:.0}", hi.y - lo.y)),
                ("depth", &format!("{:.0}", c.depth)),
            ];
            let args: Vec<(&str, &str)> = args.iter().map(|(a, b)| (*a, b.as_str())).collect();
            solidcraft_ui_text::format(if c.notch { "ui.scan_panel_notch" } else { "ui.scan_panel_inside" }, &args)
        }
        Item::Seam(i) => {
            let between = p.seams[i].between_rows;
            solidcraft_ui_text::format(
                if between { "ui.scan_panel_seam_between" } else { "ui.scan_panel_seam_beside" },
                &[("n", &count(i + 1)), ("count", &count(p.seams.len()))],
            )
        }
        Item::Hole(i) => {
            let h = &p.holes[i];
            solidcraft_ui_text::format(
                "ui.scan_panel_hole",
                &[
                    ("n", &count(i + 1)),
                    ("count", &count(p.holes.len())),
                    ("d", &format!("{:.1}", h.diameter)),
                    ("behind", &format!("{:.0}", h.behind)),
                ],
            )
        }
        Item::Fixing(i) => {
            let f = &p.fixings[i];
            solidcraft_ui_text::format(
                "ui.scan_panel_fixing",
                &[("n", &count(i + 1)), ("count", &count(p.fixings.len())), ("behind", &format!("{:.0}", f.behind))],
            )
        }
        Item::Insert(i) => {
            let o = &p.inserts[i];
            let [w, h] = o.size();
            solidcraft_ui_text::format(
                "ui.scan_panel_insert",
                &[
                    ("n", &count(i + 1)),
                    ("count", &count(p.inserts.len())),
                    ("w", &format!("{w:.0}")),
                    ("h", &format!("{h:.0}")),
                    ("depth", &format!("{:.0}", o.depth)),
                ],
            )
        }
    };
    ui_kit::text(ui, &what, ui_kit::Text::Report("scan_panel_item", k.review.dropped.contains(&item)));
    // A hole checked against the other scan: whether that has it too.
    if let (Item::Hole(i), Some((_, Some(_), seen))) = (item, &pp.confirm)
        && let Some(s) = seen.get(i)
    {
        let id = match s {
            Seen::Both => "ui.scan_panel_seen_both",
            Seen::NotFound => "ui.scan_panel_seen_not_found",
            Seen::Unscanned => "ui.scan_panel_seen_unscanned",
        };
        ui_kit::text(ui, tr(id), ui_kit::Text::Report("scan_panel_seen", *s == Seen::NotFound));
    }
    // An edge or a seam moved here.
    match item {
        Item::Edge(side) => {
            let e = p.edges;
            let mut now = [e.left - pp.base.x, e.right - pp.base.x, e.bottom - pp.base.y, e.top - pp.base.y];
            let slot = match side {
                0 => 2,
                1 => 1,
                2 => 3,
                _ => 0,
            };
            let before = now[slot];
            form_row(ui, tr("ui.scan_panel_move_to"), |ui| ui_kit::length_field(ui, &mut now[slot], &units, decimals));
            if now[slot].is_finite() && (now[slot] - before).abs() > 1e-9 {
                k.edges = Some(now);
            }
        }
        Item::Seam(i) => {
            // Moved, the holes near it and the fixing points are worked out again round it.
            let mut by = k.seam_moves.get(&i).copied().unwrap_or(0.0);
            form_row(ui, tr("ui.scan_panel_move_by"), |ui| ui_kit::length_field(ui, &mut by, &units, decimals));
            if by.is_finite() && by != k.seam_moves.get(&i).copied().unwrap_or(0.0) {
                k.seam_moves.insert(i, by);
            }
        }
        Item::Cut(i) => {
            let shapes = [Shape::Box, Shape::Straight, Shape::Scanned];
            let now = k.review.shapes.get(&i).copied().unwrap_or(p.cuts[i].shape);
            let mut pick = shapes.iter().position(|s| *s == now).unwrap_or(0);
            choice_row(
                ui,
                tr("ui.scan_panel_shape"),
                "scan_panel_shape",
                &mut pick,
                &[tr("ui.scan_panel_shape_box"), tr("ui.scan_panel_shape_straight"), tr("ui.scan_panel_shape_scanned")],
            );
            if let Some(shape) = shapes.get(pick).filter(|s| **s != now) {
                k.review.shapes.insert(i, *shape);
            }
        }
        _ => {}
    }
    // The groups, each a press away, and the whole group kept or dropped at once.
    let group = |i: &Item| match i {
        Item::Edge(_) => 0,
        Item::Cut(_) => 1,
        Item::Seam(_) => 2,
        Item::Hole(_) => 3,
        Item::Fixing(_) => 4,
        Item::Insert(_) => 5,
    };
    let names = [
        "ui.scan_panel_edges",
        "ui.scan_panel_cutouts",
        "ui.scan_panel_seams",
        "ui.scan_panel_holes",
        "ui.scan_panel_fixings",
        "ui.scan_panel_inserts",
    ];
    let mut jump: Option<usize> = None;
    ui.horizontal_wrapped(|ui| {
        for (g, name) in names.iter().enumerate() {
            let count = items.iter().filter(|i| group(i) == g).count();
            if count == 0 {
                continue;
            }
            let label = solidcraft_ui_text::format(name, &[("count", &count.to_string())]);
            let style = if group(&item) == g { ui_kit::Button::SmallSelected } else { ui_kit::Button::Small };
            if ui_kit::button(ui, label, style).clicked() {
                jump = items.iter().position(|i| group(i) == g);
            }
        }
    });
    let mut all: Option<bool> = None;
    ui.horizontal(|ui| {
        if ui_kit::button(ui, tr("ui.scan_panel_keep_all"), ui_kit::Button::Small).clicked() {
            all = Some(true);
        }
        if !matches!(item, Item::Edge(_)) && ui_kit::button(ui, tr("ui.scan_panel_drop_all"), ui_kit::Button::Small).clicked() {
            all = Some(false);
        }
    });
    if let Some(kept) = all {
        let g = group(&item);
        for i in items.iter().filter(|i| group(i) == g) {
            k.review.seen.insert(*i);
            if kept {
                k.review.dropped.remove(i);
            } else {
                k.review.dropped.insert(*i);
            }
        }
        // On to the next group, or stay at the last.
        jump = Some(items.iter().position(|i| group(i) > g).unwrap_or(k.review.at));
    }
    let mut keep = false;
    let mut drop = false;
    let mut back = false;
    let mut rest = false;
    ui.horizontal(|ui| {
        keep = ui_kit::button(ui, tr("ui.scan_panel_keep"), ui_kit::Button::Primary).clicked();
        drop = ui_kit::button(ui, tr("ui.scan_panel_drop"), ui_kit::Button::Enabled(!matches!(item, Item::Edge(_)))).clicked();
        back = ui_kit::button(ui, tr("ui.scan_panel_back"), ui_kit::Button::Enabled(k.review.at > 0)).clicked();
        rest = ui_kit::button(ui, tr("ui.scan_panel_keep_rest"), ui_kit::Button::Small).clicked();
    });
    ui_kit::text(ui, tr("ui.scan_panel_keys"), ui_kit::Text::Hint("scan_panel_keys"));
    // The keys, while no value is being typed. Enter keeps the item until all are seen; then it is OK.
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    if !typing {
        let all_seen = k.review.seen.len() >= items.len();
        ui.input_mut(|i| {
            if !all_seen && i.consume_key(egui::Modifiers::NONE, egui::Key::Enter) {
                keep = true;
            }
            keep |= i.consume_key(egui::Modifiers::NONE, egui::Key::Y);
            drop |= i.consume_key(egui::Modifiers::NONE, egui::Key::X) && !matches!(item, Item::Edge(_));
            back |= i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace);
        });
    }
    let moved_to = if let Some(at) = jump {
        Some(at)
    } else if keep || drop {
        if drop {
            k.review.dropped.insert(item);
        } else {
            k.review.dropped.remove(&item);
        }
        k.review.seen.insert(item);
        Some((k.review.at + 1).min(last))
    } else if back {
        Some(k.review.at.saturating_sub(1))
    } else if rest {
        k.review.seen.extend(items.iter().copied());
        Some(k.review.at)
    } else {
        None
    };
    if let Some(at) = moved_to {
        k.review.at = at;
        let kept = keep_of(pp, &k.review);
        k.focus = items.get(at).and_then(|i| focus_of(pp, &kept, *i));
    }
    // The pieces the seams kept make, against the sheet.
    let kept = keep_of(pp, &k.review);
    let sizes = solidcraft_engine::cmd::scan::template::panel::piece_sizes(p, &kept);
    if let Some([w, h]) = sizes.iter().copied().max_by(|a, b| (a[0] * a[1]).partial_cmp(&(b[0] * b[1])).unwrap_or(std::cmp::Ordering::Equal)) {
        let line = solidcraft_ui_text::format(
            "ui.scan_panel_pieces",
            &[("count", &sizes.len().to_string()), ("w", &format!("{w:.0}")), ("h", &format!("{h:.0}"))],
        );
        ui_kit::text(ui, &line, ui_kit::Text::Note("scan_panel_pieces"));
        if let Some(turn) = pp.prepared.squared {
            let squared = solidcraft_ui_text::format("ui.scan_panel_squared", &[("deg", &format!("{:.1}", turn.to_degrees().abs()))]);
            ui_kit::text(ui, &squared, ui_kit::Text::Note("scan_panel_squared"));
        }
        let (long, short) = (k.sheet[0].max(k.sheet[1]), k.sheet[0].min(k.sheet[1]));
        let fits = |[w, h]: [f64; 2]| (w <= long + 1e-6 && h <= short + 1e-6) || (w <= short + 1e-6 && h <= long + 1e-6);
        let over = sizes.iter().map(|&[w, h]| if fits([w, h]) { 0.0 } else { (w.max(h) - long).max(w.min(h) - short).max(0.0) }).fold(0.0, f64::max);
        if over > 0.0 {
            let warn = solidcraft_ui_text::format("ui.scan_panel_over", &[("over", &format!("{over:.0}"))]);
            ui_kit::text(ui, &warn, ui_kit::Text::Report("scan_panel_over", true));
            // The seam move that would bring every piece within the sheet, a press away.
            if let Some(&(seam, by)) = solidcraft_engine::cmd::scan::template::panel::fit_moves(p, &kept).first() {
                let fit = solidcraft_ui_text::format("ui.scan_panel_fit", &[("n", &(seam + 1).to_string()), ("by", &format!("{by:.0}"))]);
                ui_kit::text(ui, &fit, ui_kit::Text::Note("scan_panel_fit"));
                if ui_kit::button(ui, tr("ui.scan_panel_fit_apply"), ui_kit::Button::Small).clicked() {
                    *k.seam_moves.entry(seam).or_insert(0.0) += by;
                }
            }
        }
        let plans = solidcraft_engine::cmd::scan::template::panel::sheets(&sizes, k.sheet, solidcraft_engine::cmd::scan::template::panel::KERF);
        let sheets = solidcraft_ui_text::format("ui.scan_panel_sheets", &[("count", &plans.len().to_string())]);
        ui_kit::text(ui, &sheets, ui_kit::Text::Note("scan_panel_sheets"));
    }
    // The holes checked against the other scan: how they lined up, and those it lacks dropped at a press.
    if let Some((body, fit, seen)) = &pp.confirm {
        match fit {
            Some(f) => {
                let count = |w: Seen| seen.iter().filter(|s| **s == w).count().to_string();
                let line = solidcraft_ui_text::format(
                    if f.mirror { "ui.scan_panel_confirm_mirror" } else { "ui.scan_panel_confirm_same" },
                    &[
                        ("body", body),
                        ("rms", &format!("{:.1}", f.rms)),
                        ("both", &count(Seen::Both)),
                        ("not_found", &count(Seen::NotFound)),
                        ("unscanned", &count(Seen::Unscanned)),
                    ],
                );
                ui_kit::text(ui, &line, ui_kit::Text::Note("scan_panel_confirm_line"));
                let lacking: Vec<usize> = seen.iter().enumerate().filter(|(_, s)| **s == Seen::NotFound).map(|(i, _)| i).collect();
                if !lacking.is_empty() {
                    let label = solidcraft_ui_text::format("ui.scan_panel_drop_not_found", &[("count", &lacking.len().to_string())]);
                    if ui_kit::button(ui, label, ui_kit::Button::Small).clicked() {
                        for i in lacking {
                            k.review.dropped.insert(Item::Hole(i));
                            k.review.seen.insert(Item::Hole(i));
                        }
                    }
                }
            }
            None => {
                let line = solidcraft_ui_text::format("ui.scan_panel_confirm_failed", &[("body", body)]);
                ui_kit::text(ui, &line, ui_kit::Text::Report("scan_panel_confirm_failed", true));
            }
        }
    }
    if p.unbacked > 0 {
        let none = solidcraft_ui_text::format("ui.scan_panel_unbacked", &[("count", &p.unbacked.to_string())]);
        ui_kit::text(ui, &none, ui_kit::Text::Note("scan_panel_unbacked"));
    }
    if k.review.seen.len() >= items.len() {
        ui_kit::text(ui, tr("ui.scan_panel_all_seen"), ui_kit::Text::Note("scan_panel_all_seen"));
    }
    if ui_kit::button(ui, tr("ui.scan_panel_export"), ui_kit::Button::Normal).clicked() {
        k.export_asked = true;
    }
}

/// Each frame: frame the view on the item being looked at, when it changed.
pub(crate) fn tick(app: &mut SolveApp) {
    let Some(d) = app.dialog.as_mut() else { return };
    let Kind::ScanFlatTemplate(k) = &mut d.kind else { return };
    let focus = k.focus.take();
    // Export Drawings: the panel as it stands, written where the person picks.
    let export = std::mem::take(&mut k.export_asked).then(|| export_params(k, &d.inputs)).flatten();
    if let Some(bounds) = focus {
        app.fit_bounds(bounds);
    }
    if let Some(p) = export {
        app.pick_save_command("panel.dxf", &["dxf"], "scan.panel_export", p);
    }
}

/// What `scan.panel_export` is sent for the panel as the dialog has it: OK's parameters, less what
/// only the sketch takes.
fn export_params(k: &FlatTemplate, inputs: &[SelInput]) -> Option<Value> {
    let (_, mut p) = commands(k, inputs).ok()?.into_iter().find(|(c, _)| c == "scan.flat_template")?;
    if let Some(o) = p.as_object_mut() {
        for key in ["outline", "show_scan_edge", "name", "make_face"] {
            o.remove(key);
        }
    }
    Some(p)
}

/// The straight panel over the picture as it will be cut: its outline, the cut-outs dropped as dim
/// ghosts, the seams, the holes, the floor marked, and the item being looked at picked out.
fn draw_panel(painter: &egui::Painter, proj: &crate::viewport::Proj, k: &FlatTemplate, pp: &PanelPrepared) {
    let tokens = solidcraft_theme::Tokens::get();
    let keep = keep_of(pp, &k.review);
    let laid = panel_laid(pp, &keep);
    let screen = |q: Vec2| proj.to_screen(pp.at(q));
    let ring = |r: &[Vec2], stroke: egui::Stroke| {
        let pts: Vec<egui::Pos2> = r.iter().chain(r.first()).filter_map(|q| screen(*q)).collect();
        painter.add(egui::Shape::line(pts, stroke));
    };
    let p = &pp.proposal;
    for (c, kept) in p.cuts.iter().zip(&keep.cuts) {
        if !kept {
            ring(&c.ring, egui::Stroke::new(1.0, tokens.text_dim));
        }
    }
    // The openings behind the panel the insert pieces drop into, dim when dropped.
    for (i, o) in p.inserts.iter().enumerate() {
        let kept = keep.inserts.get(i).copied().unwrap_or(true);
        ring(&o.ring, egui::Stroke::new(1.0, if kept { tokens.text } else { tokens.text_dim }));
    }
    for r in &laid.inside {
        ring(r, egui::Stroke::new(1.5, tokens.error));
    }
    ring(&laid.outline, egui::Stroke::new(2.0, tokens.accent));
    for (a, b) in laid.seams.iter().flatten() {
        if let (Some(a), Some(b)) = (screen(*a), screen(*b)) {
            painter.add(egui::Shape::dashed_line(&[a, b], egui::Stroke::new(1.5, tokens.accent), 8.0, 5.0));
        }
    }
    // A hole drawn at its size, but never smaller than a dot.
    let radius = |at: Vec2, d: f64| -> Option<(egui::Pos2, f32)> {
        let c = screen(at)?;
        let edge = screen(at + Vec2::new(d / 2.0, 0.0))?;
        Some((c, c.distance(edge).max(2.5)))
    };
    let lacking = |i: usize| pp.confirm.as_ref().and_then(|(_, _, seen)| seen.get(i)).is_some_and(|s| *s == Seen::NotFound);
    for (i, h) in p.holes.iter().enumerate() {
        let kept = keep.holes.get(i).copied().unwrap_or(true);
        // A hole the other scan covers without one: in the error colour, to be looked at.
        let colour = if !kept {
            tokens.text_dim
        } else if lacking(i) {
            tokens.error
        } else {
            tokens.accent
        };
        if let Some((c, r)) = radius(h.at, h.diameter) {
            painter.circle_stroke(c, r, egui::Stroke::new(1.2, colour));
        }
    }
    // Fixing points: a filled dot, to tell them from the holes already there.
    for (i, f) in p.fixings.iter().enumerate() {
        let kept = keep.fixings.get(i).copied().unwrap_or(true);
        if let Some((c, r)) = radius(f.at, 7.0) {
            painter.circle_filled(c, r, if kept { tokens.accent } else { tokens.text_dim });
        }
    }
    // The floor: the bottom edge, named at its middle.
    let (a, b) = edge_ends(pp, 0);
    if let Some(mid) = screen((a + b) / 2.0) {
        painter.text(
            mid + egui::vec2(0.0, 14.0),
            egui::Align2::CENTER_TOP,
            tr("ui.scan_panel_floor"),
            egui::FontId::proportional(tokens.font_label),
            tokens.text,
        );
    }
    // The item being looked at.
    let items = items_of(pp);
    let Some(&item) = items.get(k.review.at) else { return };
    let mark = egui::Stroke::new(3.0, tokens.warning);
    match item {
        Item::Fixing(i) => {
            if let Some((c, r)) = p.fixings.get(i).and_then(|f| radius(f.at, 7.0)) {
                painter.circle_stroke(c, r + 4.0, mark);
            }
        }
        Item::Hole(i) => {
            if let Some((c, r)) = p.holes.get(i).and_then(|h| radius(h.at, h.diameter)) {
                painter.circle_stroke(c, r + 4.0, mark);
            }
        }
        Item::Cut(_) | Item::Insert(_) => ring(&item_points(pp, &keep, item), mark),
        _ => {
            let pts: Vec<egui::Pos2> = item_points(pp, &keep, item).into_iter().filter_map(screen).collect();
            painter.add(egui::Shape::line(pts, mark));
        }
    }
}

#[cfg(test)]
mod tests {
    /// A wall with too many triangles to paint is coloured as a grid of cells: lying on the scan
    /// (a sheet 2 mm off the template's plane, tilted 1 mm across), facing the way the scan does,
    /// only inside the outline, and no more triangles than the limit.
    #[test]
    fn a_large_wall_is_painted_as_a_grid_lying_on_the_scan() {
        let n = 80u32;
        let height = |x: f64| 2.0 + x / 100.0;
        let positions: Vec<Vec3> =
            (0..=n).flat_map(|j| (0..=n).map(move |i| (f64::from(i), f64::from(j)))).map(|(x, y)| Vec3::new(x, y, height(x))).collect();
        let mut triangles = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let k = j * (n + 1) + i;
                triangles.push([k, k + 1, k + n + 2]);
                triangles.push([k, k + n + 2, k + n + 1]);
            }
        }
        let outline = [Vec2::new(10.0, 10.0), Vec2::new(70.0, 10.0), Vec2::new(70.0, 50.0), Vec2::new(10.0, 50.0)];
        let local = |p: Vec3| Vec2::new(p.x, p.y);
        let off = |p: Vec3| p.z as f32;
        let at = |q: Vec2| Vec3::new(q.x, q.y, 0.0);
        let cells = gridded(&positions, &triangles, &outline, local, off, at, Vec3::Z);
        assert!(!cells.is_empty() && cells.len() <= PAINT_LIMIT, "{}", cells.len());
        for (corners, value) in &cells {
            let [a, b, c] = *corners;
            assert!((b - a).cross(c - a).z > 0.0, "facing the way the scan does");
            for p in corners {
                assert!((9.9..=70.1).contains(&p.x) && (9.9..=50.1).contains(&p.y), "inside the outline: {p:?}");
                assert!((p.z - height(p.x)).abs() < 0.05, "on the scan: {p:?}");
            }
            let middle = (a + b + c) / 3.0;
            assert!((f64::from(*value) - height(middle.x)).abs() < 0.05, "its colour is the scan's height there");
        }
        // Small enough to paint, the scan's own triangles are painted instead.
        assert_eq!(painted(&positions, &triangles, &outline, local, off).len(), 2 * 60 * 40);
    }

    use serde_json::json;

    use super::*;

    /// A wall 400 × 300 with an outlet box standing 25 off it, as an STL.
    fn wall_stl() -> String {
        let n = 120u32;
        let z = |x: f64, y: f64| if (100.0..=160.0).contains(&x) && (100.0..=180.0).contains(&y) { 25.0 } else { 0.0 };
        let at = |i: u32, j: u32| {
            let (x, y) = (400.0 * f64::from(i) / f64::from(n), 300.0 * f64::from(j) / f64::from(n));
            [x, y, z(x, y)]
        };
        let mut out = String::from("solid s\n");
        for j in 0..n {
            for i in 0..n {
                for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    out.push_str("facet normal 0 0 0\nouter loop\n");
                    for [x, y, zz] in tri {
                        out.push_str(&format!("vertex {x} {y} {zz}\n"));
                    }
                    out.push_str("endloop\nendfacet\n");
                }
            }
        }
        out.push_str("endsolid s\n");
        out
    }

    #[test]
    fn the_outline_needs_three_clicks() {
        let k = FlatTemplate::default();
        let none: Vec<SelInput> = vec![SelInput::new("Outline Points", FACES, true).places()];
        assert!(params(&k, &none).is_none());
    }

    /// Square To sends nothing on Automatic (the command's own default) and its choice otherwise.
    #[test]
    fn square_to_is_sent_with_the_panel() {
        let none: Vec<SelInput> = vec![SelInput::new("Outline Points", FACES, true).places()];
        let mut k = FlatTemplate { outline: 1, panel: true, ..FlatTemplate::default() };
        let sent = |k: &FlatTemplate| params(k, &none).map(|p| p["panel"]["square_to"].clone());
        assert_eq!(sent(&k), Some(Value::Null));
        k.square_to = 1;
        assert_eq!(sent(&k), Some(json!("holes")));
        k.square_to = 2;
        assert_eq!(sent(&k), Some(json!("outline")));
    }

    /// Four clicks on a wall show the template (the outlet cut out, the scan's edge) before
    /// anything changes; OK sketches it.
    #[test]
    fn clicks_on_a_wall_preview_the_template_and_ok_sketches_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("wall.stl");
        std::fs::write(&path, wall_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.flat_template");
        if let Some(d) = h.app.dialog.as_mut() {
            for (i, (x, y)) in [(20.0, 20.0), (280.0, 20.0), (280.0, 280.0), (20.0, 280.0)].into_iter().enumerate() {
                d.inputs[0].items.push(Sel::Face { body: scan.clone(), index: i, point: Vec3::new(x, y, 0.0) });
            }
        }
        h.frames(3);
        let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flat Template dialog") };
        let m = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).expect("worked out");
        assert_eq!(m.cutouts.len(), 1, "the outlet is cut out");
        assert!(!m.edge.is_empty() && m.outline.len() == 4, "the scan's edge and the outline are shown");
        assert!(h.app.session.doc.features.iter().all(|f| f.name != "Wall Template"), "nothing sketched before OK");
        // The scan over the template is coloured by how far it stands off the plane; the wall
        // outside the outlet is flat, so all of it is within the tolerance (the outlet, a cut-out
        // by design, is not counted against it).
        let (stats, tolerance) = fidelity(&h.app, k).expect("the wall measured");
        assert!(stats.count > 100 && tolerance == 0.1 && stats.within > 0.99 && stats.rms < 0.01, "{stats:?}");
        assert!(
            gpu_shade(&h.app).is_some_and(|c| c.dots.is_empty() && !c.triangles.is_empty()),
            "the scan painted over the template, its triangles, not dots"
        );
        // Once the wall's plane is known, a click on the outlet, 25 off the wall, lands on the
        // plane under it: where the template's corner will be.
        let (_, normal) = wall_plane(&h.app).expect("the wall's plane");
        assert!(normal.z.abs() > 0.999, "{normal:?}");
        let proj = crate::viewport::projection(&h.app, h.app.viewport.rect.expect("the view drawn"));
        let (at, _) = crate::scan_snap::target(&h.app, &proj, &scan, Vec3::new(130.0, 140.0, 25.0)).expect("a place on the plane");
        assert!(at.z.abs() < 0.05 && (at.x - 130.0).abs() < 5.0 && (at.y - 140.0).abs() < 5.0, "{at:?}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == "Wall Template"), "the template's sketch");
    }

    /// Whole Scan: nothing clicked, the wall's whole outline is found with the outlet cut out, no
    /// side lengths are told over it, and OK sketches it.
    #[test]
    fn whole_scan_finds_the_outline_without_clicks() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("wall.stl");
        std::fs::write(&path, wall_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.flat_template");
        if let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.outline = 1;
            k.smooth = 0.0;
            k.round = 0.0;
            k.side = 1;
            k.edge = false;
        }
        h.frames(3);
        let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flat Template dialog") };
        let m = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).expect("worked out");
        assert_eq!(m.cutouts.len(), 1, "the outlet is cut out");
        assert!(m.step.is_some() && m.sides.is_empty() && m.edge.is_empty(), "a found outline: no side lengths, no ragged edge");
        assert!((m.area - 400.0 * 300.0).abs() < 0.02 * 120_000.0, "the whole wall: {}", m.area);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == "Wall Template"), "the template's sketch");
    }

    /// Straight Panel: the proposal is looked through one thing at a time (the four edges, then the
    /// outlet's cut-out); dropping the outlet sends it as dropped, and OK sketches the panel.
    #[test]
    fn a_straight_panel_is_looked_through_and_ok_makes_what_was_kept() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("wall.stl");
        std::fs::write(&path, wall_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.flat_template");
        if let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.outline = 1;
            k.smooth = 0.0;
            k.round = 0.0;
            k.side = 1;
            k.edge = false;
            k.panel = true;
        }
        h.frames(3);
        let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flat Template dialog") };
        let pp = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).and_then(|m| m.panel.clone()).expect("a proposal");
        let items = items_of(&pp);
        assert_eq!(items[..5], [Item::Edge(0), Item::Edge(1), Item::Edge(2), Item::Edge(3), Item::Cut(0)], "the edges and the outlet");
        // Then the fixing points: the wall is flat, so the edges with no hole get them.
        assert!(items.len() > 5 && items[5..].iter().all(|i| matches!(i, Item::Fixing(_))), "{items:?}");
        assert!(focus_of(&pp, &keep_of(&pp, &k.review), Item::Cut(0)).is_some_and(|b| b.max.x - b.min.x >= 2.0 * FOCUS_ROOM), "room round it");
        if let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.review.dropped.insert(Item::Cut(0));
            k.review.seen.extend(items.iter().copied());
            k.review.at = items.len() - 1;
        }
        h.frames(1);
        let d = h.app.dialog.as_ref().expect("still open");
        let Kind::ScanFlatTemplate(k) = &d.kind else { panic!("the Flat Template dialog") };
        let sent = commands(k, &d.inputs).expect("OK runs");
        let (_, p) = sent.last().expect("the template");
        assert_eq!(p["panel"]["drop_cutouts"], json!([0]), "{p}");
        // Export Drawings sends the same panel to Export Panel, less what only the sketch takes.
        let e = export_params(k, &d.inputs).expect("export parameters");
        assert!(e.get("outline").is_none() && e["panel"]["drop_cutouts"] == json!([0]), "{e}");
        let exported = h.app.session.execute("scan.panel_export", &e).expect("exported");
        assert!(exported["pieces"].as_array().is_some_and(|p| !p.is_empty()), "{exported}");
        // All looked at: Enter is OK again.
        h.step(&json!({"key": "Enter"})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == "Wall Template"), "the panel's sketch");
    }

    /// With a second scan in the design, Check Holes Against is offered; checked against a scan
    /// whose holes do not line up (these walls have none), the panel is still proposed and says so.
    #[test]
    fn a_second_scan_is_offered_to_check_the_holes_against() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("wall.stl");
        std::fs::write(&path, wall_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let names: Vec<String> = h.app.session.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        h.app.run("select.set", json!({"items": [Sel::Body { name: names[0].clone() }]})).unwrap();
        h.app.start("scan.flat_template");
        if let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.outline = 1;
            k.smooth = 0.0;
            k.round = 0.0;
            k.side = 1;
            k.edge = false;
            k.panel = true;
            k.confirm_with = Some(names[1].clone());
        }
        h.frames(3);
        let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flat Template dialog") };
        let shown = k.shown.as_ref().map(|s| s.result.as_ref().map(|_| ()).map_err(Clone::clone));
        let pp =
            k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).and_then(|m| m.panel.clone()).unwrap_or_else(|| panic!("a proposal: {shown:?}"));
        let (body, fit, _) = pp.confirm.as_ref().expect("checked");
        assert!(body == &names[1] && fit.is_none(), "no holes to line up");
        let d = h.app.dialog.as_ref().expect("open");
        let (_, p) = commands(k, &d.inputs).expect("OK runs").pop().expect("the template");
        assert_eq!(p["panel"]["confirm_with"], json!(names[1]), "{p}");
    }
}
