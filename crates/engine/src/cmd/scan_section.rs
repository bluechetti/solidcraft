//! Section to Sketch (`scan.section_sketch`): where a plane cuts the scan, as a sketch of lines,
//! arcs and circles fitted to the cut (SurfaceCraft's chain fitter): each piece averages the scan's
//! points, so its noise is not traced, the pieces meet end to end, and smooth joins are held
//! tangent. The plane is the sketch being edited, or a new sketch's; the result is ordinary sketch
//! geometry to dimension and extrude.
//!
//! Sections Along Curve (`scan.sections_along_curve`): planes square to a curve of a 3D sketch at
//! even steps along it cut the scan, each section drawn the same way in a sketch of its own on its
//! plane, in order along the curve, ready to loft.

use serde_json::{Value, json};
use solidcraft_geom::prelude::*;
use solidcraft_scan::Scan;
use solidcraft_scan::chain;
pub use solidcraft_scan::chain::{Outline, Piece};
pub use solidcraft_scan::section::Along;
use solidcraft_sketch::{ConstraintKind, Sketch};

use super::{length, mesh_name};
use crate::params::{bad, bool_, string_list_or_default};
use crate::schema::scan::{ScanSectionResult, ScanSectionsAlongResult};
use crate::{EngineError, Result, Session};

/// The least default tolerance: how far a piece may stray from the cut's points (the fixed default
/// before the default followed the cut's noise, and still a clean scan's).
pub const TOLERANCE: f64 = 0.1;

/// The default tolerance is this many times the cut's noise, so a fit averages the noise rather
/// than tracing it ...
const NOISE_TIMES: f64 = 3.0;

/// ... and at most this share of the cut's size.
const MOST_SHARE: f64 = 0.01;

/// A cut's noise, as a standard deviation: from the middle of how far each of its points lies off
/// the line through its two neighbours (a straight or gently curving cut puts them on it; the
/// scan's noise puts them off). `None` with too few points to tell.
pub fn cut_noise(runs: &[solidcraft_scan::section::Run]) -> Option<f64> {
    let mut off = Vec::new();
    for r in runs {
        let n = r.points.len();
        if n < 3 {
            continue;
        }
        let inner = if r.closed { 0..n } else { 1..n - 1 };
        for i in inner {
            let (a, p, b) = (r.points[(i + n - 1) % n], r.points[i], r.points[(i + 1) % n]);
            let ab = b - a;
            let len = ab.len();
            if len > 0.0 {
                off.push(((p - a).cross(ab) / len).abs());
            }
        }
    }
    if off.len() < 16 {
        return None;
    }
    let middle = off.len() / 2;
    off.select_nth_unstable_by(middle, f64::total_cmp);
    // A point's offset from its neighbours' chord, for noise of deviation σ at each point, has a
    // median of about 0.83 σ.
    Some(off[middle] / 0.83)
}

/// The tolerance a cut is fitted at when none is given: [`NOISE_TIMES`] its noise, at most
/// [`MOST_SHARE`] of its size, and never under [`TOLERANCE`]. A clean scan's cut gets the old
/// 0.1 mm; a noisy or very large scan's is not traced point by point.
pub fn noise_tolerance(runs: &[solidcraft_scan::section::Run]) -> f64 {
    let mut points = runs.iter().flat_map(|r| r.points.iter().copied());
    let Some(first) = points.next() else { return TOLERANCE };
    let (lo, hi) =
        points.fold(((first.x, first.y), (first.x, first.y)), |(lo, hi), p| ((lo.0.min(p.x), lo.1.min(p.y)), (hi.0.max(p.x), hi.1.max(p.y))));
    let size = (hi.0 - lo.0).hypot(hi.1 - lo.1);
    cut_noise(runs).map_or(TOLERANCE, |noise| (NOISE_TIMES * noise).min(MOST_SHARE * size).max(TOLERANCE))
}

/// The tolerance `p` asks for (`tolerance`), `None` when it leaves it to the cut.
fn asked_tolerance(s: &Session, p: &Value, cmd: &str) -> Result<Option<f64>> {
    match crate::params::expr(p, "tolerance") {
        Some(_) => Ok(Some(length(s, p, "tolerance", TOLERANCE, cmd)?)),
        None => Ok(None),
    }
}

/// The name of a sketch Section to Sketch makes.
const SKETCH_NAME: &str = "Section";

/// Unasked, an outline is fitted to within this share of the scan's size ...
const OUTLINE_SHARE: f64 = 0.002;
/// ... and how far the scan reaches outside it is read on this many of its points.
const OUTLINE_SAMPLE: usize = 50_000;

/// A fitted outline as a ring of points, each arc as sixteen chords.
fn ring_of(outline: &Outline) -> Vec<solidcore_record::Vec2> {
    use solidcore_record::Vec2;
    match outline {
        Outline::Circle { center, radius } => {
            (0..64).map(|k| std::f64::consts::TAU * f64::from(k) / 64.0).map(|a| *center + Vec2::new(a.cos(), a.sin()) * *radius).collect()
        }
        Outline::Chain(pieces) => pieces
            .iter()
            .flat_map(|piece| match *piece {
                Piece::Line { from, .. } => vec![from],
                Piece::Arc { center, from, to, ccw, .. } => {
                    let (u, v) = (from - center, to - center);
                    let mut turn = u.cross(v).atan2(u.dot(v));
                    if ccw && turn <= 0.0 {
                        turn += std::f64::consts::TAU;
                    } else if !ccw && turn >= 0.0 {
                        turn -= std::f64::consts::TAU;
                    }
                    (0..16)
                        .map(|k| {
                            let a = turn * f64::from(k) / 16.0;
                            center + Vec2::new(u.x * a.cos() - u.y * a.sin(), u.x * a.sin() + u.y * a.cos())
                        })
                        .collect()
                }
            })
            .collect(),
    }
}

/// A half profile's outline kept on its side of the axis (the sketch's X, y = 0), so that a
/// Revolve about it takes it: the fit moves joins by up to its tolerance, which can put a corner
/// meant to be on the axis a hair off it or below it, and bulge an arc across it. Each join within
/// `reach` of the axis, or below it, is put on it, and an arc that still dips below it is drawn
/// straight.
fn held_to_axis(outline: &mut Outline, reach: f64) {
    let Outline::Chain(pieces) = outline else { return };
    let onto = |p: &mut solidcore_record::Vec2| {
        if p.y <= reach {
            p.y = 0.0;
        }
    };
    for piece in pieces.iter_mut() {
        match piece {
            Piece::Line { from, to } | Piece::Arc { from, to, .. } => {
                onto(from);
                onto(to);
            }
        }
    }
    for piece in pieces.iter_mut() {
        let Piece::Arc { center, from, to, ccw, .. } = *piece else { continue };
        let (u, v) = (from - center, to - center);
        let mut turn = u.cross(v).atan2(u.dot(v));
        if ccw && turn <= 0.0 {
            turn += std::f64::consts::TAU;
        } else if !ccw && turn >= 0.0 {
            turn -= std::f64::consts::TAU;
        }
        let dips = (1..16).any(|k| {
            let a = turn * f64::from(k) / 16.0;
            center.y + u.x * a.sin() + u.y * a.cos() < -1e-9
        });
        if dips {
            *piece = Piece::Line { from, to };
        }
    }
}

/// The name of the sketch it makes of a turned part's half profile.
const HALF_NAME: &str = "Half Profile";

/// What Sections Along Curve names its sketches, before their number.
const ALONG_NAME: &str = "Curve Section";

/// How many sections Sections Along Curve cuts unless told.
pub const SECTIONS: usize = 8;

/// How near the curve a cut must come to be kept, as a share of the scan's size.
const ALONG_REACH: f64 = 0.1;

/// Joins whose directions differ by less than this are held tangent.
const SMOOTH: f64 = 3.0 * std::f64::consts::PI / 180.0;

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value> {
    super::super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
}

/// How [`finish_dimensioned`] left the sketch.
#[derive(Debug, Default)]
pub(super) struct Finished {
    /// The degrees of freedom left: after AutoConstrain, or as the sketch solved when it could
    /// not be dimensioned; `None` when it was finished undimensioned as asked.
    pub(super) dof: Option<usize>,
    /// Why it was not dimensioned, and what it was left with.
    pub(super) warning: Option<String>,
}

/// Finishes the sketch being edited as Finish with AutoConstrain does when `dimensions`: its
/// lines and arcs held where the scan put them by dimensions measured from the sketch's origin,
/// each a parameter, so the profile follows a changed parameter as a sketch drawn by hand does.
/// With `round`, each dimension within it of a round size takes that size (AutoConstrain's
/// `round`). A sketch AutoConstrain cannot settle is finished as it is, with its degrees of
/// freedom and a warning saying why; with `dimensions` false it is finished as it is.
pub(super) fn finish_dimensioned(s: &mut Session, dimensions: bool, round: Option<f64>) -> Result<Finished> {
    if !dimensions {
        run(s, "sketch.finish", json!({}))?;
        return Ok(Finished::default());
    }
    let asked = match round {
        Some(reach) => json!({"round": reach}),
        None => json!({}),
    };
    let why = match run(s, "sketch.finish_auto_constrain", asked) {
        Ok(reply) => return Ok(Finished { dof: reply.get("dof").and_then(Value::as_u64).and_then(|d| usize::try_from(d).ok()), warning: None }),
        Err(e) => e.to_string(),
    };
    // Said, not swallowed: a caller reading only the reply must see it is not dimensioned.
    let solved = run(s, "sketch.solve", json!({})).ok();
    let dof = solved.as_ref().and_then(|r| r.get("dof")).and_then(Value::as_u64).and_then(|d| usize::try_from(d).ok());
    let status = solved.as_ref().and_then(|r| r.get("status")).and_then(Value::as_str).unwrap_or("unknown").to_owned();
    run(s, "sketch.finish", json!({}))?;
    let left = dof.map_or_else(String::new, |d| format!(", {d} degrees of freedom left"));
    Ok(Finished { dof, warning: Some(format!("not dimensioned ({why}); finished as drawn, solve status {status}{left}")) })
}

/// What was added to the sketch.
#[derive(Default)]
pub(super) struct Added {
    pub(super) lines: usize,
    pub(super) arcs: usize,
    pub(super) circles: usize,
    pub(super) tangents: usize,
}

/// One closed or open chain added to the sketch, its ends shared and its smooth joins tangent.
fn add_chain(sk: &mut Sketch, pieces: &[Piece], closed: bool, added: &mut Added) -> Result<()> {
    let mut ends: Vec<usize> = pieces.iter().map(|p| sk.add_point(p.from(), None)).collect::<std::result::Result<_, _>>()?;
    match (closed, pieces.last()) {
        (true, _) => ends.push(ends[0]),
        (false, Some(last)) => ends.push(sk.add_point(last.to(), None)?),
        (false, None) => return Ok(()),
    }
    let mut curves = Vec::with_capacity(pieces.len());
    for (k, piece) in pieces.iter().enumerate() {
        let (a, b) = (ends[k], ends[k + 1]);
        let curve = match *piece {
            Piece::Arc { center, from, to, ccw, .. } => {
                // Sketch arcs run counter-clockwise: a clockwise one is the same arc from its end.
                let made = if ccw {
                    sk.add_arc(center, from, to, [None, Some(a), Some(b)], None)
                } else {
                    sk.add_arc(center, to, from, [None, Some(b), Some(a)], None)
                };
                match made {
                    Ok(c) => {
                        added.arcs += 1;
                        c
                    }
                    Err(_) => {
                        added.lines += 1;
                        sk.add_line_pts(a, b, None)?
                    }
                }
            }
            Piece::Line { .. } => {
                added.lines += 1;
                sk.add_line_pts(a, b, None)?
            }
        };
        curves.push(curve);
    }
    // Smooth joins held tangent (where an arc meets anything; two lines in line are left alone).
    let count = pieces.len();
    let joins = if closed { count } else { count.saturating_sub(1) };
    for k in 0..joins {
        let next = (k + 1) % count;
        let (here, there) = (&pieces[k], &pieces[next]);
        let arcy = matches!(here, Piece::Arc { .. }) || matches!(there, Piece::Arc { .. });
        if let (true, Some(u), Some(v)) = (arcy, here.direction(true), there.direction(false))
            && u.dot(v).clamp(-1.0, 1.0).acos() < SMOOTH
            && sk.add_constraint(ConstraintKind::Tangent { a: curves[k], b: curves[next] }, None).is_ok()
        {
            added.tangents += 1;
        }
    }
    Ok(())
}

/// The outlines of a cut, each closed or not, drawn into the sketch.
pub(super) fn draw(sk: &mut Sketch, outlines: &[(Outline, bool)], added: &mut Added) -> Result<()> {
    let from = sk.constraints.len();
    for (outline, closed) in outlines {
        match outline {
            Outline::Circle { center, radius } => {
                sk.add_circle(*center, *radius, None, None)?;
                added.circles += 1;
            }
            Outline::Chain(pieces) => add_chain(sk, pieces, *closed, added)?,
        }
    }
    added.tangents = added.tangents.saturating_sub(unfold(sk, from));
    Ok(())
}

/// Most rounds [`unfold`] solves.
const UNFOLD_ROUNDS: usize = 8;

/// Drops the tangents from constraint `from` on that the sketch cannot hold without folding over:
/// a fitted join a degree or two off tangent between long shallow arcs (a car panel's section)
/// is made tangent by sliding its touch points by the radius times that angle, which can run a
/// short line between them backwards. The solver refuses that fold and leaves the whole sketch
/// unsolved, so AutoConstrain cannot dimension it; such a join stays a shared end instead. How
/// many were dropped.
fn unfold(sk: &mut Sketch, from: usize) -> usize {
    let mut dropped = 0;
    for _ in 0..UNFOLD_ROUNDS {
        let mut trial = sk.clone();
        let rep = solidcraft_sketch::solve_with(&mut trial, &solidcraft_sketch::Hold::default(), None, solidcraft_sketch::Analysis::StatusOnly, None);
        let ours = |c: &solidcraft_sketch::Constraint| matches!(c.kind, ConstraintKind::Tangent { .. }) && rep.folded.contains(&c.id);
        let before = sk.constraints.len();
        let mut k = 0;
        sk.constraints.retain(|c| {
            k += 1;
            k <= from || !ours(c)
        });
        let gone = before - sk.constraints.len();
        if gone == 0 {
            break;
        }
        dropped += gone;
    }
    dropped
}

/// The outlines fitted to a cut's runs, leaving out specks shorter than a few tolerances.
fn fitted(runs: &[solidcraft_scan::section::Run], tolerance: f64) -> Vec<(Outline, bool)> {
    let length_of = |r: &solidcraft_scan::section::Run| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
    runs.iter()
        .filter(|r| length_of(r) > 10.0 * tolerance)
        .filter_map(|r| chain::chain(&r.points, r.closed, tolerance).map(|o| (o, r.closed)))
        .collect()
}

/// The half profile's plane and side, when `p` gives a turned part's `axis`: the plane through
/// the axis as `sketch.create` takes one (its origin the axis's, its X along it, its Y the
/// direction square to it nearest level, turned about it by `angle` degrees), and the side of it
/// to draw (1 or -1 along Y).
fn half_plane(s: &Session, p: &Value, cmd: &str) -> Result<Option<(Value, f64)>> {
    let Some(axis) = p.get("axis").filter(|v| !v.is_null()) else {
        if p.get("side").is_some_and(|v| !v.is_null()) || p.get("angle").is_some_and(|v| !v.is_null()) {
            return Err(bad(cmd, "`side` and `angle` go with `axis` (a turned part's half profile)"));
        }
        return Ok(None);
    };
    if super::super::sketch_store::active(s).is_some() {
        return Err(bad(cmd, "`axis` makes a sketch of its own on a plane through the axis: finish the sketch being edited first"));
    }
    if p.get("plane").is_some_and(|v| !v.is_null()) || p.get("offset").is_some_and(|v| !v.is_null()) {
        return Err(bad(cmd, "`axis` gives the plane (through the axis): leave `plane` and `offset` out, and turn it with `angle`"));
    }
    let point = |key: &str| axis.get(key).and_then(crate::params::vec3).filter(|v| v.is_finite());
    let origin = point("origin").ok_or_else(|| bad(cmd, "`axis.origin` must be a point [x, y, z] of numbers"))?;
    let along = point("dir").and_then(|d| d.normalized()).ok_or_else(|| bad(cmd, "`axis.dir` must be a direction [x, y, z], not zero"))?;
    let side = match crate::params::str_(p, "side") {
        None | Some("positive") => 1.0,
        Some("negative") => -1.0,
        Some(other) => return Err(bad(cmd, format!("`side` is `positive` or `negative`, not `{other}`"))),
    };
    let angle = match p.get("angle").filter(|v| !v.is_null()) {
        Some(v) => v
            .as_f64()
            .filter(|a| a.is_finite() && a.abs() <= 360.0)
            .ok_or_else(|| bad(cmd, "`angle` is a number of degrees, at most 360"))?
            .to_radians(),
        None => 0.0,
    };
    // Square to the axis and as level as that allows; for an upright axis, along the world's X.
    let level = solidcore_record::Vec3::Z.cross(along).normalized().unwrap_or(solidcore_record::Vec3::X);
    let across = level * angle.cos() + along.cross(level) * angle.sin();
    let at = |v: solidcore_record::Vec3| [v.x, v.y, v.z];
    Ok(Some((json!({"origin": at(origin), "x_dir": at(along), "y_dir": at(across)}), side)))
}

pub(super) fn section_sketch(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.section_sketch";
    let asked = asked_tolerance(s, p, cmd)?;
    let scan = mesh_name(s, p, cmd)?;
    let half = half_plane(s, p, cmd)?;
    // The sketch being edited, or a new one on the plane asked for.
    let (sketch, made) = match super::super::sketch_store::active(s) {
        Some(id) => (id, false),
        None if half.is_some() => {
            let plane = half.as_ref().map(|(plane, _)| plane.clone()).unwrap_or_default();
            let reply = super::new_sketch(s, json!({"plane": plane, "name": HALF_NAME}))?;
            (reply.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the sketch was not made"))?, true)
        }
        None => {
            let mut create = json!({"plane": p.get("plane").cloned().unwrap_or(json!("XY")), "name": SKETCH_NAME});
            if let Some(offset) = p.get("offset").filter(|v| !v.is_null()) {
                create["offset"] = offset.clone();
            }
            let reply = super::new_sketch(s, create)?;
            (reply.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the sketch was not made"))?, true)
        }
    };
    if s.model.state().sketch(sketch).is_none() {
        s.refresh();
    }
    // A sketch the user is editing is left to them; one made here is finished dimensioned.
    let dimensions = bool_(p, "dimensions").unwrap_or(true);
    let finish = |s: &mut Session, round: f64| if made { finish_dimensioned(s, dimensions, Some(round)) } else { Ok(Finished::default()) };
    let runs = {
        let state = s.model.state();
        let plane = state.sketch(sketch).map(|ss| ss.plane).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
        super::with_shown(s, &scan, |p, t| solidcraft_scan::section::section(&Scan::new(p, t), &plane))
            .ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?
    };
    // The outline of all the scan covers of the plane, not where the plane cuts it.
    let whole = bool_(p, "outline").unwrap_or(false);
    if whole && half.is_some() {
        return Err(bad(cmd, "`outline` and `axis` are two different sketches: ask for one"));
    }
    let (runs, outline_tolerance) = if whole {
        let state = s.model.state();
        let plane = state.sketch(sketch).map(|ss| ss.plane).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?;
        let covered = super::with_shown(s, &scan, |p, t| {
            let scan = Scan::new(p, t);
            // As fine as the tolerance asked for; unasked, a five-hundredth of the scan's size.
            let tolerance = asked.unwrap_or_else(|| (OUTLINE_SHARE * scan.extent()).max(TOLERANCE));
            let (runs, cell) = solidcraft_scan::silhouette::outline(&scan, &plane, 0.5 * tolerance);
            // The outline runs along cells: fitted no finer than two of them, their steps are averaged.
            (runs, tolerance.max(2.0 * cell))
        })
        .ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
        (covered.0, Some(covered.1))
    } else {
        (runs, None)
    };
    let tolerance = outline_tolerance.unwrap_or_else(|| asked.unwrap_or_else(|| noise_tolerance(&runs)));
    // About an axis: one side of the cut, closed along the axis where it comes down to it.
    let (runs, sides) = match &half {
        Some((_, side)) => {
            let kept = solidcraft_scan::section::half(&runs, *side, 2.0 * tolerance);
            let (closed, open) = (kept.iter().filter(|k| k.1).count(), kept.iter().filter(|k| !k.1).count());
            let runs =
                kept.into_iter().map(|(points, closed)| solidcraft_scan::section::Run { triangles: vec![0; points.len()], points, closed }).collect();
            (runs, Some((closed, open)))
        }
        None => (runs, None),
    };
    let mut outlines = fitted(&runs, tolerance);
    if half.is_some() {
        for (outline, _) in &mut outlines {
            held_to_axis(outline, tolerance);
        }
    }
    if outlines.is_empty() {
        let note = match (runs.is_empty(), half.is_some()) {
            (true, true) => "the plane through the axis does not cut the scan on that side: check the axis, or try the other `side`",
            (true, false) => "the plane does not cut the scan",
            (false, _) => "the cut is too small to sketch",
        };
        if made {
            run(s, "sketch.cancel", json!({}))?;
        }
        return Err(bad(cmd, note));
    }
    // How far the scan reaches outside the outline as fitted (its lines and arcs average the
    // cells' steps, so a little of it can): read on a sample of its points.
    let outside = whole
        .then(|| {
            let state = s.model.state();
            let plane = state.sketch(sketch).map(|ss| ss.plane)?;
            let rings: Vec<Vec<solidcore_record::Vec2>> = outlines.iter().map(|(o, _)| ring_of(o)).collect();
            let outer = rings.first()?;
            super::with_shown(s, &scan, |p, _| {
                let step = (p.len() / OUTLINE_SAMPLE).max(1);
                let flat: Vec<solidcore_record::Vec2> = p
                    .iter()
                    .step_by(step)
                    .map(|q| {
                        let d = *q - plane.origin;
                        solidcore_record::Vec2::new(d.dot(plane.x), d.dot(plane.y))
                    })
                    // A scrap beside the part has an outline of its own: its points are in that one.
                    .filter(|q| !rings.iter().skip(1).any(|r| solidcraft_scan::profile::area(r) > 0.0 && solidcraft_scan::profile::contains(r, *q)))
                    .collect();
                solidcraft_scan::silhouette::farthest_outside(outer, &flat)
            })
        })
        .flatten();
    let mut added = Added::default();
    let drawn = super::super::sketch_store::update(s, sketch, |_, _, sk| draw(sk, &outlines, &mut added));
    if let Err(e) = drawn {
        if made {
            run(s, "sketch.cancel", json!({}))?;
        }
        return Err(e);
    }
    // Sizes within a quarter of the cut's tolerance of a round size are read as designed, as
    // Hole It reads a hole's.
    let finished = finish(s, solidcraft_scan::hole_form::SNAP_SHARE * tolerance)?;
    let reply = ScanSectionResult {
        body: scan,
        sketch,
        outlines: outlines.len(),
        lines: added.lines,
        arcs: added.arcs,
        circles: added.circles,
        tangents: added.tangents,
        tolerance,
        dof: finished.dof,
        warning: finished.warning,
        outside,
        closed_on_axis: sides.map(|(closed, _)| closed),
        left_open: sides.map(|(_, open)| open),
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The curve Sections Along Curve follows: the 3D sketch (`sketch`, by id or name; default the
/// design's last `Scan Curves`), the curves (`curves`; default its last), and their points end to
/// end, each turned round to run on from the one before.
pub fn along_path(s: &Session, sketch: Option<&Value>, curves: &[String], cmd: &str) -> Result<(u64, Vec<String>, Vec<solidcore_record::Vec3>)> {
    let sketch = match sketch.filter(|v| !v.is_null()) {
        Some(v) => super::super::features::sketch_id(s, Some(v), cmd, "sketch")?,
        None => s
            .doc
            .features
            .iter()
            .rev()
            .find(|f| f.name == super::curve::SKETCH_NAME && matches!(f.kind, solidcraft_doc::FeatureKind::Sketch { .. }))
            .map(|f| f.id)
            .ok_or_else(|| bad(cmd, "no curve to follow: draw one with Curve on Scan, or give `sketch` and `curves`"))?,
    };
    let state = s.model.state();
    let wires = &state.sketch(sketch).ok_or_else(|| bad(cmd, "the sketch is not evaluated"))?.sketch.wires;
    let curves: Vec<String> = if curves.is_empty() {
        let last = wires.iter().rev().find(|w| w.pts.len() >= 2).ok_or_else(|| bad(cmd, "the sketch has no 3D curve to follow"))?;
        vec![last.id.clone()]
    } else {
        curves.to_vec()
    };
    if curves.len() > 1000 {
        return Err(bad(cmd, "`curves` lists at most 1000 curves"));
    }
    let mut path: Vec<solidcore_record::Vec3> = Vec::new();
    for id in &curves {
        let wire = wires.iter().find(|w| w.id == *id).ok_or_else(|| bad(cmd, format!("no 3D curve `{id}` in the sketch")))?;
        let mut pts = wire.pts.clone();
        if let (Some(end), Some(first), Some(last)) = (path.last(), pts.first(), pts.last())
            && end.dist(*last) < end.dist(*first)
        {
            pts.reverse();
        }
        let skip = usize::from(path.last().zip(pts.first()).is_some_and(|(a, b)| a.dist(*b) < 1e-9));
        path.extend(pts.into_iter().skip(skip));
    }
    if path.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() <= 0.0 {
        return Err(bad(cmd, "the curve has no length to cut along"));
    }
    Ok((sketch, curves, path))
}

/// The sections of the scan `body` along `path`, as Sections Along Curve cuts them.
pub fn along_cut(s: &Session, body: &str, path: &[solidcore_record::Vec3], count: usize, every: bool, cmd: &str) -> Result<Vec<Along>> {
    super::with_shown(s, body, |p, t| {
        let scan = Scan::new(p, t);
        solidcraft_scan::section::along(&scan, path, count, every, ALONG_REACH * scan.extent())
    })
    .ok_or_else(|| bad(cmd, format!("`{body}` is not a scan")))
}

/// What the command would cut, for a dialog's preview: the scan, the curve's points, and the
/// sections with the outlines fitted to each.
pub fn along_preview(
    s: &Session,
    body: Option<&str>,
    sketch: Option<u64>,
    curves: &[String],
    count: usize,
    every: bool,
    tolerance: Option<f64>,
) -> Result<(String, Vec<solidcore_record::Vec3>, Vec<(Along, Vec<Outline>)>, f64)> {
    let cmd = "scan.sections_along_curve";
    let scan = mesh_name(s, &json!({ "body": body }), cmd)?;
    let (_, _, path) = along_path(s, sketch.map(|id| json!(id)).as_ref(), curves, cmd)?;
    let cut = along_cut(s, &scan, &path, count.clamp(1, 60), every, cmd)?;
    let tolerance = tolerance.unwrap_or_else(|| along_tolerance(&cut));
    let sections = cut
        .into_iter()
        .map(|a| {
            let outlines = fitted(&a.runs, tolerance).into_iter().map(|(o, _)| o).collect();
            (a, outlines)
        })
        .collect();
    Ok((scan, path, sections, tolerance))
}

/// One tolerance for all the sections along a curve, from all their cuts' noise.
fn along_tolerance(sections: &[Along]) -> f64 {
    let runs: Vec<solidcraft_scan::section::Run> = sections.iter().flat_map(|a| a.runs.iter().cloned()).collect();
    noise_tolerance(&runs)
}

pub(super) fn sections_along_curve(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.sections_along_curve";
    let asked = asked_tolerance(s, p, cmd)?;
    let count = match p.get("count").filter(|v| !v.is_null()) {
        None => SECTIONS,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=60).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| bad(cmd, "`count` must be a whole number of sections from 1 to 60"))?,
    };
    let every = bool_(p, "every").unwrap_or(false);
    let scan = mesh_name(s, p, cmd)?;
    let (_, _, path) = along_path(s, p.get("sketch"), &string_list_or_default(p, "curves"), cmd)?;
    let length_along: f64 = path.windows(2).map(|w| w[0].dist(w[1])).sum();
    let sections = along_cut(s, &scan, &path, count, every, cmd)?;
    let tolerance = asked.unwrap_or_else(|| along_tolerance(&sections));
    let planned: Vec<(solidcore_record::Plane, Vec<(Outline, bool)>)> =
        sections.iter().map(|a| (a.station.plane, fitted(&a.runs, tolerance))).filter(|(_, o)| !o.is_empty()).collect();
    if planned.is_empty() {
        return Err(bad(cmd, "no plane square to the curve cuts the scan near it: put the curve on the scan, or through the middle of the part"));
    }
    let missed = sections.len() - planned.len();
    // Numbered on from the sections already in the design.
    let taken: std::collections::BTreeSet<&str> = s.doc.features.iter().map(|f| f.name.as_str()).collect();
    let mut names = Vec::with_capacity(planned.len());
    let mut number = 1;
    while names.len() < planned.len() {
        let name = format!("{ALONG_NAME} {number}");
        if !taken.contains(name.as_str()) {
            names.push(name);
        }
        number += 1;
    }
    let mut added = Added::default();
    let mut sketches = Vec::with_capacity(planned.len());
    let at = |v: solidcore_record::Vec3| [v.x, v.y, v.z];
    for ((plane, outlines), name) in planned.iter().zip(&names) {
        let create = json!({"plane": {"origin": at(plane.origin), "x_dir": at(plane.x), "y_dir": at(plane.y)}, "name": name});
        let made = super::new_sketch(s, create)?;
        let id = made.get("sketch").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "the sketch was not made"))?;
        if s.model.state().sketch(id).is_none() {
            s.refresh();
        }
        if let Err(e) = super::super::sketch_store::update(s, id, |_, _, sk| draw(sk, outlines, &mut added)) {
            run(s, "sketch.cancel", json!({}))?;
            return Err(e);
        }
        run(s, "sketch.finish", json!({}))?;
        sketches.push(id);
    }
    let reply = ScanSectionsAlongResult {
        body: scan,
        sketches,
        missed,
        outlines: planned.iter().map(|(_, o)| o.len()).sum(),
        lines: added.lines,
        arcs: added.arcs,
        circles: added.circles,
        tangents: added.tangents,
        length: length_along,
        tolerance,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The plane `sketch.create` makes for "XY", "XZ" or "YZ" moved `offset` along its normal (XY:
/// x, y along +X, +Y; XZ: +X, −Z; YZ: +Y, +Z), for a dialog to preview the cut on.
pub fn standard_plane(name: &str, offset: f64) -> Option<solidcore_record::Plane> {
    use solidcore_record::{Plane, Vec3};
    let plane = match name {
        "XY" => Plane { origin: Vec3::ZERO, x: Vec3::X, y: Vec3::Y },
        "XZ" => Plane { origin: Vec3::ZERO, x: Vec3::X, y: -Vec3::Z },
        "YZ" => Plane { origin: Vec3::ZERO, x: Vec3::Y, y: Vec3::Z },
        _ => return None,
    };
    offset.is_finite().then(|| plane.offset(offset))
}

/// The cut the command would sketch, for a dialog's preview: the scan, the plane (the sketch's
/// being edited, else `plane`), and the outlines fitted to the cut.
pub fn preview(
    s: &Session,
    body: Option<&str>,
    plane: solidcore_record::Plane,
    tolerance: Option<f64>,
) -> Result<(String, solidcore_record::Plane, Vec<Outline>, f64)> {
    let cmd = "scan.section_sketch";
    let scan = mesh_name(s, &json!({ "body": body }), cmd)?;
    let state = s.model.state();
    let plane = super::super::sketch_store::active(s).and_then(|id| state.sketch(id)).map_or(plane, |ss| ss.plane);
    let runs = super::with_shown(s, &scan, |p, t| solidcraft_scan::section::section(&Scan::new(p, t), &plane))
        .ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
    let tolerance = tolerance.unwrap_or_else(|| noise_tolerance(&runs));
    let outlines = runs
        .iter()
        .filter(|r| r.points.windows(2).map(|w| w[0].dist(w[1])).sum::<f64>() > 10.0 * tolerance)
        .filter_map(|r| chain::chain(&r.points, r.closed, tolerance))
        .collect();
    Ok((scan, plane, outlines, tolerance))
}
