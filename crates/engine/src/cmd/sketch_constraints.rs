//! More sketch constraints (curvature, polygon), constraint glyph anchors and snapping
//! inference for interactive tools.

use serde_json::Value;
use solidcore_record::Vec2;
use solidcraft_geom::prelude::*;
use solidcraft_sketch::{ConstraintKind, CurveKind, Sketch};

use super::sketch::{add_c, dof_now, edit, keep_redundant, listed, redundancy, residual_of};
use super::sketch_store;
use super::{CommandSpec, in_sketch};
use crate::params::{bad, num, req_vec2, str_, string_list, vec2};
use crate::schema::sketch_constraints::{
    AutoConstrainDatumParams, AutoConstrainParams, AutoConstrainResult, ConstrainerParams, ConstrainerResult, ConstraintAdded, CurvatureParams,
    FinishAutoConstrainParams, Glyph, GlyphsParams, GlyphsResult, PolygonParams, PolygonResult, Snap, SnapParams, SnapResult, ToggleDiameterParams,
    ToggleDiameterResult, ToggleDrivenParams, ToggleDrivenResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const TOGGLE_DIAMETER_CONTRACT: TypedContract<ToggleDiameterParams> = TypedContract::new();
const CURVATURE_CONTRACT: TypedContract<CurvatureParams> = TypedContract::new();
const POLYGON_CONTRACT: TypedContract<PolygonParams> = TypedContract::new();
const AUTO_CONSTRAIN_CONTRACT: TypedContract<AutoConstrainParams> = TypedContract::new();
const AUTO_CONSTRAIN_DATUM_CONTRACT: TypedContract<AutoConstrainDatumParams> = TypedContract::new();
const FINISH_AUTO_CONSTRAIN_CONTRACT: TypedContract<FinishAutoConstrainParams> = TypedContract::new();
const CONSTRAINER_CONTRACT: TypedContract<ConstrainerParams> = TypedContract::new();
const TOGGLE_DRIVEN_CONTRACT: TypedContract<ToggleDrivenParams> = TypedContract::new();
const GLYPHS_CONTRACT: TypedContract<GlyphsParams> = TypedContract::new();
const SNAP_CONTRACT: TypedContract<SnapParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.constraint.curvature", "Curvature", crate::checked!("sketch.constraint.curvature", c_smooth))
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_smooth")
        .enabled(in_sketch)
        .contract(&CURVATURE_CONTRACT)
        .output(output_schema_for::<ConstraintAdded>)
        .params("a, b: two curves sharing an end point (curvature continuous, G2)"),
    CommandSpec::new("sketch.constraint.polygon", "Polygon", crate::checked!("sketch.constraint.polygon", c_polygon))
        .at("SKETCH", "CONSTRAINTS")
        .icon("c_polygon")
        .enabled(in_sketch)
        .contract(&POLYGON_CONTRACT)
        .output(output_schema_for::<PolygonResult>)
        .params("lines: [line ids] of a closed chain: equal sides with every corner on one circle (a regular polygon)"),
    CommandSpec::new("sketch.auto_constrain", "AutoConstrain", crate::checked!("sketch.auto_constrain", auto_constrain))
        .at("SKETCH", "CONSTRAINTS")
        .icon("auto_constrain")
        .enabled(in_sketch)
        .contract(&AUTO_CONSTRAIN_CONTRACT)
        .output(output_schema_for::<AutoConstrainResult>)
        .params("tolerance?: mm (default 0.01), angle_tolerance?: deg (default 1), dimensions?: bool (default true): infer constraints, then dimension until fully constrained; round?: mm: each dimension within this of a whole, half or tenth of the design unit takes it, when no point moves farther than this (the value measured kept in its parameter's comment)"),
    CommandSpec::new("sketch.auto_constrain_from_datum", "AutoConstrain from Datum", crate::checked!("sketch.auto_constrain_from_datum", auto_constrain_datum))
        .at("SKETCH", "CONSTRAINTS")
        .icon("auto_constrain")
        .enabled(in_sketch)
        .contract(&AUTO_CONSTRAIN_DATUM_CONTRACT)
        .output(output_schema_for::<AutoConstrainResult>)
        .params("datum: point ref (default origin); like AutoConstrain with positions measured from the datum; round?: mm: each dimension within this of a whole, half or tenth of the design unit takes it, when no point moves farther than this (the value measured kept in its parameter's comment)"),
    CommandSpec::new("sketch.finish_auto_constrain", "Finish with AutoConstrain", crate::checked!("sketch.finish_auto_constrain", auto_constrain_finish))
        .at("SKETCH", "FINISH SKETCH")
        .icon("finish")
        .enabled(in_sketch)
        .contract(&FINISH_AUTO_CONSTRAIN_CONTRACT)
        .output(output_schema_for::<AutoConstrainResult>)
        .params("like AutoConstrain, then Finish Sketch; round?: mm: each dimension within this of a whole, half or tenth of the design unit takes it, when no point moves farther than this (the value measured kept in its parameter's comment)"),
    CommandSpec::new("sketch.constrainer", "Sketch Constrainer", crate::checked!("sketch.constrainer", smart_constrain))
        .at("SKETCH", "CREATE")
        .icon("constrainer")
        .enabled(in_sketch)
        .contract(&CONSTRAINER_CONTRACT)
        .output(output_schema_for::<ConstrainerResult>)
        .params("entities: [one or two sketch entity ids]: applies the constraint the geometry nearly has (horizontal/vertical, parallel, perpendicular, collinear, coincident, concentric, equal, tangent)"),
    CommandSpec::new("sketch.toggle_driven", "Toggle Driven Dimension", crate::checked!("sketch.toggle_driven", toggle_driven)).icon("dimension")
        .enabled(in_sketch)
        .contract(&TOGGLE_DRIVEN_CONTRACT)
        .output(output_schema_for::<ToggleDrivenResult>)
        .params("constraint: dimension id, driven?: bool (default toggles); a driving dimension gets a parameter with its current value"),
    CommandSpec::new("sketch.toggle_diameter", "Toggle Radius / Diameter", crate::checked!("sketch.toggle_diameter", toggle_diameter))
        .icon("dimension")
        .enabled(in_sketch)
        .contract(&TOGGLE_DIAMETER_CONTRACT)
        .output(output_schema_for::<ToggleDiameterResult>)
        .params("constraint: a circle's or an arc's radius or diameter dimension id; diameter?: bool (default: the other one); sketch?: id|name (default: the one being edited). The circle stays the same size: its parameter is halved or doubled (a plain value is rewritten, an expression becomes `(e) / 2` or `2 * (e)`); refused when anything else uses that parameter"),
    CommandSpec::new("sketch.glyphs", "Constraint Glyphs", crate::checked!("sketch.glyphs", glyphs)).icon("eye")
        .enabled(in_sketch)
        .noundo().read_only()
        .contract(&GLYPHS_CONTRACT)
        .output(output_schema_for::<GlyphsResult>)
        .params("sketch?: id|name: every constraint with where to draw its glyph (sketch coordinates)"),
    CommandSpec::new("sketch.snap", "Snap Inference", crate::checked!("sketch.snap", snap)).icon("snap")
        .enabled(in_sketch)
        .noundo().read_only()
        .contract(&SNAP_CONTRACT)
        .output(output_schema_for::<SnapResult>)
        .params("at: [x,y], from?: [x,y] (previous click), radius?: mm (snap distance, default 1): what a click there would snap to and infer"),
];

fn c_smooth(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.curvature";
    let (mut before, mut residual) = (None, None);
    let (id, info) = edit(s, p, cmd, true, |sk, _| {
        let a = curve(sk, p.get("a"), cmd)?;
        let b = curve(sk, p.get("b"), cmd)?;
        before = Some(dof_now(sk));
        let id = add_c(sk, ConstraintKind::Smooth { a, b })?;
        residual = residual_of(sk, &id);
        Ok(id)
    })?;
    let by = redundancy(s, p, cmd, before, &info, residual, &id)?;
    let (redundant, determined_by) = redundant_part(p, cmd, by)?;
    reply(cmd, ConstraintAdded { constraint: id, sketch: info, redundant, determined_by, created: None, param_warnings: Vec::new() })
}

/// A command's reply as JSON: they build their replies from the types their output schemas
/// describe, so the two can't drift apart.
fn reply(cmd: &str, reply: impl serde::Serialize) -> Result<Value> {
    serde_json::to_value(reply).map_err(|e| bad(cmd, e.to_string()))
}

/// What a new constraint's reply says of redundancy: whether it is, and by which constraints. A
/// redundant one is refused when the call asked for `if_redundant: "refuse"`.
fn redundant_part(p: &Value, cmd: &str, by: Option<Vec<String>>) -> Result<(bool, Option<Vec<String>>)> {
    match by {
        Some(by) => {
            keep_redundant(p, cmd, &by)?;
            Ok((true, Some(by)))
        }
        None => Ok((false, None)),
    }
}

fn curve(sk: &Sketch, v: Option<&Value>, cmd: &str) -> Result<usize> {
    let id = v.and_then(Value::as_str).ok_or_else(|| bad(cmd, "expected a curve id"))?;
    sk.curve_index(id).ok_or_else(|| bad(cmd, format!("unknown curve `{id}`")))
}

fn c_polygon(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constraint.polygon";
    let ids = string_list(cmd, p, "lines")?;
    if ids.len() < 3 || ids.len() > 200 {
        return Err(bad(cmd, "`lines` must list 3 to 200 lines of a closed chain"));
    }
    let (out, info) = edit(s, p, cmd, true, |sk, _| {
        let ls: Vec<usize> = ids.iter().map(|i| sk.curve_index(i).ok_or_else(|| bad(cmd, format!("unknown curve `{i}`")))).collect::<Result<_>>()?;
        let mut corners: Vec<usize> = Vec::new();
        for l in &ls {
            match sk.curves.get(*l).map(|c| &c.kind) {
                Some(CurveKind::Line { a, b }) => {
                    for q in [*a, *b] {
                        if !corners.contains(&q) {
                            corners.push(q);
                        }
                    }
                }
                _ => return Err(bad(cmd, "the polygon must be made of lines")),
            }
        }
        if corners.len() != ls.len() {
            return Err(bad(cmd, "the lines must form one closed chain"));
        }
        let pts: Vec<Vec2> = corners.iter().filter_map(|q| sk.point(*q)).collect();
        let c = pts.iter().fold(Vec2::ZERO, |a, q| a + *q) / pts.len() as f64;
        let r = pts.iter().map(|q| q.dist(c)).sum::<f64>() / pts.len() as f64;
        let circ = sk.add_circle(c, r.max(1e-6), None, None)?;
        if let Some(cu) = sk.curves.get_mut(circ) {
            cu.construction = true;
        }
        let mut cons = Vec::new();
        for w in ls.windows(2) {
            cons.push(add_c(sk, ConstraintKind::Equal { a: w[0], b: w[1] })?);
        }
        for q in corners {
            cons.push(add_c(sk, ConstraintKind::PointOnCurve { p: q, c: circ })?);
        }
        Ok(cons)
    })?;
    reply(cmd, PolygonResult { constraints: out, sketch: info, created: None, param_warnings: Vec::new() })
}

/// Most constraints AutoConstrain tries.
const MAX_TRIALS: usize = 2000;

/// Add `k` if it removes a degree of freedom and the sketch still solves; else leave the
/// sketch as it was. Returns the new dof when kept.
fn try_add(sk: &mut Sketch, doc: &mut solidcraft_doc::Document, k: ConstraintKind, dof: usize, dim: bool) -> Option<usize> {
    let saved = sk.clone();
    let param = if dim {
        let v = k.value()?;
        let (unit, e) = if k.is_angle() { ("deg".into(), format!("{} deg", (v.to_degrees() * 1e6).round() / 1e6)) } else { doc.design_length(v) };
        Some(doc.new_model_param(&e, &unit))
    } else {
        None
    };
    if sk.add_constraint(k, param.clone()).is_err() {
        if let Some(pn) = &param {
            let _ = doc.remove_param(pn);
        }
        return None;
    }
    let mut trial = sk.clone();
    let rep = solidcraft_sketch::solve_with(&mut trial, &solidcraft_sketch::Hold::default(), None, solidcraft_sketch::Analysis::Freedom, None);
    if rep.ok() && rep.dof < dof {
        *sk = trial;
        return Some(rep.dof);
    }
    *sk = saved;
    if let Some(pn) = &param {
        let _ = doc.remove_param(pn);
    }
    None
}

/// Adds the dimensions `sizes` (each holding where the sketch is) that remove a degree of
/// freedom, in order and until none is left, as one [`try_add`] after another would, but with
/// one rank walk ([`solidcraft_sketch::adds_rank`]) and one solve to confirm. The sizes still to
/// try one at a time: none when the solve agrees with the walk, those not kept when it leaves
/// degrees of freedom the walk counted as removed. `None`, with the sketch and parameters as
/// they were, when the walk cannot say or the confirming solve fails, so the caller tries them
/// all one at a time.
fn dimension_at_once(sk: &mut Sketch, doc: &mut solidcraft_doc::Document, sizes: &[ConstraintKind], dof: &mut usize) -> Option<Vec<ConstraintKind>> {
    if *dof == 0 || sizes.is_empty() {
        return Some(Vec::new());
    }
    let saved = sk.clone();
    let from = sk.constraints.len();
    // (constraint id, parameter) of each one added.
    let mut made: Vec<(String, String)> = Vec::new();
    let forget = |doc: &mut solidcraft_doc::Document, made: &[(String, String)]| {
        for (_, pn) in made {
            let _ = doc.remove_param(pn);
        }
    };
    // The size each constraint added stands for.
    let mut of_size: Vec<usize> = Vec::new();
    for (i, k) in sizes.iter().enumerate() {
        let Some(v) = k.value() else { continue };
        let (unit, e) = if k.is_angle() { ("deg".into(), format!("{} deg", (v.to_degrees() * 1e6).round() / 1e6)) } else { doc.design_length(v) };
        let pn = doc.new_model_param(&e, &unit);
        match sk.add_constraint(k.clone(), Some(pn.clone())) {
            Ok(id) => {
                made.push((id, pn));
                of_size.push(i);
            }
            Err(_) => {
                let _ = doc.remove_param(&pn);
            }
        }
    }
    let Some(adds) = solidcraft_sketch::adds_rank(sk, from) else {
        *sk = saved;
        forget(doc, &made);
        return None;
    };
    // Kept in order until the degrees of freedom run out.
    let mut keep = vec![false; made.len()];
    let mut kept = 0;
    for (k, slot) in keep.iter_mut().enumerate() {
        if kept == *dof {
            break;
        }
        if adds.get(k).copied().unwrap_or(false) {
            *slot = true;
            kept += 1;
        }
    }
    let dropped: Vec<(String, String)> = made.iter().zip(&keep).filter(|(_, k)| !**k).map(|(m, _)| m.clone()).collect();
    sk.constraints.retain(|c| !dropped.iter().any(|(id, _)| *id == c.id));
    let mut trial = sk.clone();
    let rep = solidcraft_sketch::solve_with(&mut trial, &solidcraft_sketch::Hold::default(), None, solidcraft_sketch::Analysis::Freedom, None);
    if !(rep.ok() && rep.dof < *dof) {
        *sk = saved;
        forget(doc, &made);
        return None;
    }
    // Where the rows are nearly dependent (a near-degenerate join of a CAD outline) the solver's
    // rank can leave a degree of freedom the walk counted as removed: the sizes not kept are
    // then tried one at a time, and the solver's count is the one that stands.
    let rest: Vec<ConstraintKind> = if rep.dof + kept == *dof {
        Vec::new()
    } else {
        made.iter().zip(&keep).zip(&of_size).filter(|((_, k), _)| !**k).filter_map(|(_, i)| sizes.get(*i).cloned()).collect()
    };
    forget(doc, &dropped);
    *sk = trial;
    *dof = rep.dof;
    Some(rest)
}

fn run_auto(sk: &mut Sketch, doc: &mut solidcraft_doc::Document, p: &Value, datum: usize, mut dof: usize) -> (Vec<String>, usize) {
    use ConstraintKind::*;
    let tol = num(p, "tolerance").filter(|t| *t > 0.0).unwrap_or(0.01);
    let atol = num(p, "angle_tolerance").filter(|t| *t > 0.0).unwrap_or(1.0).to_radians();
    let dims = crate::params::bool_(p, "dimensions").unwrap_or(true);
    let n0 = sk.constraints.len();
    let trials = std::cell::Cell::new(0_usize);
    let attempt = |sk: &mut Sketch, doc: &mut solidcraft_doc::Document, k: ConstraintKind, dim: bool, dof: &mut usize| {
        if *dof == 0 || trials.get() >= MAX_TRIALS {
            return;
        }
        trials.set(trials.get() + 1);
        if let Some(d) = try_add(sk, doc, k, *dof, dim) {
            *dof = d;
        }
    };
    // Coincident points.
    let np = sk.points.len();
    for i in 0..np {
        for j in (i + 1)..np {
            let (Some(a), Some(b)) = (sk.point(i), sk.point(j)) else { continue };
            if a.dist(b) <= tol {
                attempt(sk, doc, Coincident { p: i, q: j }, false, &mut dof);
            }
        }
    }
    // Horizontal / vertical lines, tangent line–arc and arc–arc joints.
    let nc = sk.curves.len();
    for i in 0..nc {
        if let Some(CurveKind::Line { a, b }) = sk.curves.get(i).map(|c| c.kind.clone())
            && let (Some(pa), Some(pb)) = (sk.point(a), sk.point(b))
        {
            let ang = (pb - pa).angle();
            let off_h = ang.sin().abs().asin();
            let off_v = ang.cos().abs().asin();
            if off_h <= atol {
                attempt(sk, doc, Horizontal { l: i }, false, &mut dof);
            } else if off_v <= atol {
                attempt(sk, doc, Vertical { l: i }, false, &mut dof);
            }
        }
    }
    for i in 0..nc {
        for j in (i + 1)..nc {
            let (Some(ci), Some(cj)) = (sk.curves.get(i), sk.curves.get(j)) else { continue };
            let round = |k: &CurveKind| matches!(k, CurveKind::Arc { .. } | CurveKind::Circle { .. });
            if !(round(&ci.kind) || round(&cj.kind)) {
                continue;
            }
            let (Some((a0, a1)), Some((b0, b1))) = (ci.kind.ends(), cj.kind.ends()) else { continue };
            let Some(sh) = [a0, a1].into_iter().find(|x| *x == b0 || *x == b1) else { continue };
            let dir = |c: usize| -> Option<Vec2> {
                let k = sk.curves.get(c)?.kind.clone();
                let q = sk.point(sh)?;
                match k {
                    CurveKind::Line { a, b } => (sk.point(b)? - sk.point(a)?).normalized(),
                    CurveKind::Arc { c, .. } => (q - sk.point(c)?).perp().normalized(),
                    _ => None,
                }
            };
            if let (Some(u), Some(v)) = (dir(i), dir(j))
                && u.cross(v).abs() <= atol.sin()
            {
                attempt(sk, doc, Tangent { a: i, b: j }, false, &mut dof);
            }
        }
    }
    if dims {
        // Construction curves have degrees of freedom too: locating a circle's centre
        // does not constrain its radius. Dimension them just like profile curves.
        let mut sizes = Vec::new();
        for i in 0..nc {
            let k = match sk.curves.get(i).map(|c| c.kind.clone()) {
                Some(CurveKind::Circle { r, .. }) => Diameter { c: i, value: 2.0 * r },
                Some(CurveKind::Arc { .. }) => Radius { c: i, value: sk.radius(i).unwrap_or(1.0) },
                Some(CurveKind::Line { a, b }) => Length { l: i, value: sk.point(a).zip(sk.point(b)).map(|(x, y)| x.dist(y)).unwrap_or(1.0) },
                _ => continue,
            };
            sizes.push(k);
        }
        // Positions from the datum.
        let d = sk.point(datum).unwrap_or_default();
        for i in 0..sk.points.len() {
            if i == datum {
                continue;
            }
            let Some(q) = sk.point(i) else { continue };
            sizes.push(DistanceX { p: datum, q: i, value: (q.x - d.x).abs() });
            sizes.push(DistanceY { p: datum, q: i, value: (q.y - d.y).abs() });
        }
        // Each holds where the sketch is, so which of them remove a degree of freedom is one
        // walk over the Jacobian rather than a solve each (minutes on a long scan section). The
        // walk costs no trials: MAX_TRIALS bounds solves, and a section of 600 curves has more
        // sizes than that, which left it hundreds of degrees of freedom short.
        let rest = dimension_at_once(sk, doc, &sizes, &mut dof).unwrap_or(sizes);
        for k in rest {
            attempt(sk, doc, k, true, &mut dof);
        }
    }
    if let Some(reach) = num(p, "round").filter(|r| *r > 0.0 && *r < 1e9) {
        round_dimensions(sk, doc, n0, reach);
    }
    let added = sk.constraints.iter().skip(n0).map(|c| c.id.clone()).collect();
    (added, dof)
}

/// The dimensions from constraint `from` on read as designed: each set to the roundest of a
/// whole, a half and a tenth of the design's length unit within `reach` of it, when the sketch
/// still solves the same way round and no point of it ends farther than `reach` from where it
/// was. Its parameter's comment keeps the value measured. How many were rounded.
/// Work the rounding of one AutoConstrain may spend, as solver iterations times the cube of the
/// sketch's coordinates (what a dense iteration costs): about ten seconds. A hand-drawn sketch
/// of 50 points rounds every size well inside it; a scan section of 1000 points rounds what a
/// few dozen iterations allow, and the rest keep their measured values.
const ROUND_WORK: f64 = 4e11;

fn round_dimensions(sk: &mut Sketch, doc: &mut solidcraft_doc::Document, from: usize, reach: f64) -> usize {
    let per_unit = match solidcraft_doc::expr::unit_info(doc.units.trim()) {
        Some((solidcraft_doc::expr::Kind::Length, s)) if s.is_finite() && s > 0.0 => s,
        _ => 1.0,
    };
    let was: Vec<Option<Vec2>> = (0..sk.points.len()).map(|i| sk.point(i)).collect();
    // Each dimension within `reach` of a round size: (constraint, round size, measured, parameter).
    let mut asked: Vec<(usize, f64, f64, String)> = Vec::new();
    for k in from..sk.constraints.len() {
        let Some(c) = sk.constraints.get(k) else { continue };
        let (Some(name), Some(value)) = (c.param.clone(), c.kind.value()) else { continue };
        if c.driven || c.kind.is_angle() {
            continue;
        }
        let designed = [1.0, 2.0, 10.0]
            .into_iter()
            .map(|per| (value / per_unit * per).round() / per * per_unit)
            .find(|v| (v - value).abs() <= reach)
            .filter(|v| *v > 0.0 && (v - value).abs() > 1e-9);
        if let Some(designed) = designed
            && doc.params.iter().any(|q| q.name == name)
        {
            asked.push((k, designed, value, name));
        }
    }
    // A size whose rounding would move the sketch more than `reach` even to first order is left
    // as measured without a solve: on a long scan section (shallow arcs, hundreds of sizes) such
    // a solve crawls to its iteration limit, and those took most of a quarter of an hour.
    let cube = ((2 * sk.points.len()).max(1) as f64).powi(3);
    if cube > ROUND_WORK {
        return 0;
    }
    if let Some(moves) = solidcraft_sketch::first_order_moves(sk, &asked.iter().map(|(k, d, ..)| (*k, *d)).collect::<Vec<_>>()) {
        let mut near = moves.into_iter();
        asked.retain(|_| near.next().is_some_and(|m| m <= reach));
    }
    let mut kept: Vec<&(usize, f64, f64, String)> = Vec::new();
    let mut spent = 0.0_f64;
    for one in &asked {
        // What is left is kept as measured once the rounding has had its share of the solver.
        if spent >= ROUND_WORK {
            break;
        }
        let (k, designed, ..) = one;
        let saved = sk.clone();
        if let Some(c) = sk.constraints.get_mut(*k) {
            c.kind.set_value(*designed);
        }
        let rep = solidcraft_sketch::solve_with(sk, &solidcraft_sketch::Hold::default(), None, solidcraft_sketch::Analysis::StatusOnly, None);
        spent += rep.iterations.max(1) as f64 * cube;
        let stays = was.iter().enumerate().all(|(i, a)| a.zip(sk.point(i)).is_none_or(|(a, b)| a.dist(b) <= reach));
        if rep.ok() && rep.keeps_orientation() && stays {
            kept.push(one);
        } else {
            *sk = saved;
        }
    }
    for (_, designed, value, name) in &kept {
        // The parameter is written in place: Document::set_param copies the whole design, scans
        // and all, for each one.
        let (unit, expr) = doc.design_length(*designed);
        let measured = doc.design_length(*value).1;
        if let Some(q) = doc.params.iter_mut().find(|q| q.name == *name) {
            q.expr = expr;
            q.unit = unit;
            q.comment = format!("measured {measured}");
        }
    }
    kept.len()
}

fn auto(s: &mut Session, p: &Value, cmd: &str) -> Result<Value> {
    let ((added, dof), info) = edit(s, p, cmd, false, |sk, doc| {
        let datum = match str_(p, "datum") {
            Some(r) => sk.resolve_point(r).ok_or_else(|| bad(cmd, format!("unknown point `{r}`")))?,
            None => 0,
        };
        // Values of existing dimensions must be applied (and reached from the stored solved
        // state) before judging degrees of freedom; a sketch they would turn over keeps its
        // stored geometry, as the edit's own solve then refuses them.
        let rep = super::sketch::solve_current(sk, doc)?;
        if !rep.keeps_orientation() {
            return Err(bad(cmd, rep.failure(|f| format!("the sketch constraints conflict ({f})"))));
        }
        // This is the exact state just solved for the current parameter values. Trials update
        // its freedom whenever they change constraints; no separate initial solve is needed.
        Ok(run_auto(sk, doc, p, datum, rep.dof))
    })?;
    // The degrees of freedom of the sketch as stored (the edit's own solve, after rounding),
    // which is what Inspect reports later; the trials' count stands in when it did not solve.
    let dof = info.get("dof").and_then(Value::as_u64).and_then(|d| usize::try_from(d).ok()).unwrap_or(dof);
    reply(cmd, AutoConstrainResult { added, dof, sketch: info, created: None, param_warnings: Vec::new() })
}

fn auto_constrain(s: &mut Session, p: &Value) -> Result<Value> {
    auto(s, p, "sketch.auto_constrain")
}
fn auto_constrain_datum(s: &mut Session, p: &Value) -> Result<Value> {
    auto(s, p, "sketch.auto_constrain_from_datum")
}
fn auto_constrain_finish(s: &mut Session, p: &Value) -> Result<Value> {
    let r = auto(s, p, "sketch.finish_auto_constrain")?;
    s.active_sketch = None;
    s.revision += 1;
    Ok(r)
}

/// The constraint two entities (or one line) are closest to satisfying.
fn guess(sk: &Sketch, ents: &[String], cmd: &str) -> Result<ConstraintKind> {
    use ConstraintKind::*;
    let ent = |r: &str| -> Result<(Option<usize>, Option<usize>)> {
        if let Some(c) = sk.curve_index(r) {
            return Ok((None, Some(c)));
        }
        sk.resolve_point(r).map(|p| (Some(p), None)).ok_or_else(|| bad(cmd, format!("unknown sketch entity `{r}`")))
    };
    let line_dir = |c: usize| match sk.shape(c) {
        Some(solidcraft_sketch::Shape::Line { a, b }) => (b - a).normalized(),
        _ => None,
    };
    let round = |c: usize| matches!(sk.curves.get(c).map(|c| &c.kind), Some(CurveKind::Circle { .. } | CurveKind::Arc { .. }));
    match ents {
        [a] => {
            let (_, Some(c)) = ent(a)? else { return Err(bad(cmd, "pick a line, or two entities")) };
            let d = line_dir(c).ok_or_else(|| bad(cmd, "a single entity must be a line"))?;
            Ok(if d.x.abs() >= d.y.abs() { Horizontal { l: c } } else { Vertical { l: c } })
        }
        [a, b] => match (ent(a)?, ent(b)?) {
            ((Some(p), _), (Some(q), _)) => Ok(Coincident { p, q }),
            ((Some(p), _), (_, Some(c))) | ((_, Some(c)), (Some(p), _)) => {
                // A point at a curve's centre is concentric-like: coincident with the centre.
                if let Some(cp) = sk.curves.get(c).and_then(|cu| match cu.kind {
                    CurveKind::Circle { c, .. } | CurveKind::Arc { c, .. } => Some(c),
                    _ => None,
                }) && sk.point(cp).zip(sk.point(p)).is_some_and(|(x, y)| x.dist(y) < sk.radius(c).unwrap_or(1.0) * 0.25)
                {
                    return Ok(Coincident { p, q: cp });
                }
                Ok(PointOnCurve { p, c })
            }
            ((_, Some(x)), (_, Some(y))) => {
                if let (Some(u), Some(v)) = (line_dir(x), line_dir(y)) {
                    let cross = u.cross(v).abs();
                    if cross < 0.2 {
                        // Parallel; on one line → collinear.
                        let off = match (sk.shape(x), sk.shape(y)) {
                            (Some(solidcraft_sketch::Shape::Line { a, .. }), Some(solidcraft_sketch::Shape::Line { a: b0, .. })) => {
                                u.cross(b0 - a).abs()
                            }
                            _ => f64::INFINITY,
                        };
                        let len = sk.polyline(x).windows(2).map(|w| w[0].dist(w[1])).sum::<f64>();
                        return Ok(if off < len * 0.05 { Collinear { a: x, b: y } } else { Parallel { a: x, b: y } });
                    }
                    if u.dot(v).abs() < 0.2 {
                        return Ok(Perpendicular { a: x, b: y });
                    }
                    return Ok(Equal { a: x, b: y });
                }
                if round(x) && round(y) {
                    let (cx, cy) = (sk.center(x).unwrap_or_default(), sk.center(y).unwrap_or_default());
                    let (rx, ry) = (sk.radius(x).unwrap_or(1.0), sk.radius(y).unwrap_or(1.0));
                    if cx.dist(cy) < 0.25 * rx.min(ry) {
                        return Ok(Concentric { a: x, b: y });
                    }
                    let d = cx.dist(cy);
                    if (d - (rx + ry)).abs() < 0.15 * rx.min(ry) || (d - (rx - ry).abs()).abs() < 0.15 * rx.min(ry) {
                        return Ok(Tangent { a: x, b: y });
                    }
                    return Ok(Equal { a: x, b: y });
                }
                Ok(Tangent { a: x, b: y })
            }
            _ => Err(bad(cmd, "cannot constrain that combination")),
        },
        _ => Err(bad(cmd, "`entities` must list one or two entities")),
    }
}

fn smart_constrain(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.constrainer";
    let ents = string_list(cmd, p, "entities")?;
    let (mut before, mut residual) = (None, None);
    let mut name = "";
    let (id, info) = edit(s, p, cmd, true, |sk, _| {
        let k = guess(sk, &ents, cmd)?;
        name = k.name();
        before = Some(dof_now(sk));
        let id = add_c(sk, k)?;
        residual = residual_of(sk, &id);
        Ok(id)
    })?;
    let by = redundancy(s, p, cmd, before, &info, residual, &id)?;
    let (redundant, determined_by) = redundant_part(p, cmd, by)?;
    reply(
        cmd,
        ConstrainerResult {
            constraint: id,
            sketch: info,
            redundant,
            determined_by,
            kind: name.to_string(),
            created: None,
            param_warnings: Vec::new(),
        },
    )
}

fn toggle_driven(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.toggle_driven";
    let cid = str_(p, "constraint").ok_or_else(|| bad(cmd, "`constraint` must be a dimension id"))?.to_string();
    let want = crate::params::bool_(p, "driven");
    let mut before = None;
    let (driven, info) = edit(s, p, cmd, true, |sk, doc| {
        let c = sk.constraints.iter().find(|c| c.id == cid).cloned().ok_or_else(|| bad(cmd, format!("no constraint `{cid}`")))?;
        let v = c.kind.value().ok_or_else(|| bad(cmd, format!("`{cid}` is not a dimension")))?;
        let driven = want.unwrap_or(!c.driven);
        if driven == c.driven {
            return Ok(driven);
        }
        let slot = sk.constraints.iter_mut().find(|c| c.id == cid).ok_or_else(|| bad(cmd, "constraint"))?;
        if driven {
            slot.driven = true;
            slot.param = None;
        } else {
            let (unit, e) =
                if c.kind.is_angle() { ("deg".into(), format!("{} deg", (v.to_degrees() * 1e6).round() / 1e6)) } else { doc.design_length(v) };
            slot.driven = false;
            slot.param = Some(doc.new_model_param(&e, &unit));
            before = None;
        }
        Ok(driven)
    })?;
    if !driven {
        // Making it driving must not over-constrain: compare with the sketch without it.
        let id = sketch_store::active(s).ok_or_else(|| bad(cmd, "no sketch"))?;
        let mut sk = sketch_store::read(&s.doc, id)?.clone();
        if let Some(c) = sk.constraints.iter_mut().find(|c| c.id == cid) {
            c.driven = true;
        }
        before = Some(dof_now(&sk));
        // It holds (its parameter takes the value it measures), so it can only be redundant.
        if let Some(by) = redundancy(s, p, cmd, before, &info, Some(0.0), &cid)? {
            return Err(bad(cmd, format!("that dimension is already determined by {}; it stays driven", listed(&by))));
        }
    }
    s.doc_mut().prune_model_params();
    reply(cmd, ToggleDrivenResult { constraint: cid, driven, sketch: info, created: None, param_warnings: Vec::new() })
}

fn toggle_diameter(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.toggle_diameter";
    let cid = str_(p, "constraint").ok_or_else(|| bad(cmd, "`constraint` must be a dimension id"))?.to_string();
    let want = crate::params::bool_(p, "diameter");
    let sketch = sketch_store::target(s, p, cmd)?;
    let (diameter, info) = edit(s, p, cmd, true, |sk, doc| {
        let c = sk.constraints.iter().find(|c| c.id == cid).cloned().ok_or_else(|| bad(cmd, format!("no constraint `{cid}`")))?;
        let round = |curve: usize| matches!(sk.curves.get(curve).map(|c| &c.kind), Some(CurveKind::Circle { .. } | CurveKind::Arc { .. }));
        let (curve, value, now) = match c.kind {
            ConstraintKind::Radius { c, value } if round(c) => (c, value, false),
            ConstraintKind::Diameter { c, value } if round(c) => (c, value, true),
            _ => return Err(bad(cmd, format!("`{cid}` is not a circle's or an arc's radius or diameter dimension"))),
        };
        let diameter = want.unwrap_or(!now);
        if diameter == now {
            return Ok(diameter);
        }
        let (kind, value) = if diameter {
            (ConstraintKind::Diameter { c: curve, value: 2.0 * value }, 2.0 * value)
        } else {
            (ConstraintKind::Radius { c: curve, value: value / 2.0 }, value / 2.0)
        };
        // Its parameter now holds the other measure, so the circle keeps its size; another user
        // of that parameter would change size with it.
        if let Some(name) = &c.param {
            let others: Vec<String> = doc
                .param_users(name)
                .into_iter()
                .filter(
                    |u| !matches!(u, solidcraft_doc::ParamUser::Dimension { feature, constraint, .. } if *feature == sketch && *constraint == cid),
                )
                .map(|u| u.describe())
                .collect();
            if !others.is_empty() {
                return Err(bad(cmd, format!("`{name}` is also used by {}; switching `{cid}` would change them", others.join(", "))));
            }
            let e = doc.param(name).map(|p| p.expr.clone()).ok_or_else(|| bad(cmd, format!("no parameter `{name}`")))?;
            let e = if solidcraft_doc::expr::references(&e).is_empty() {
                doc.design_length(value).1
            } else if diameter {
                format!("2 * ({e})")
            } else {
                format!("({e}) / 2")
            };
            doc.set_param(name, &e, None, None).map_err(|e| bad(cmd, e.to_string()))?;
        }
        let slot = sk.constraints.iter_mut().find(|c| c.id == cid).ok_or_else(|| bad(cmd, "constraint"))?;
        slot.kind = kind;
        Ok(diameter)
    })?;
    let reply = ToggleDiameterResult { constraint: cid, diameter, sketch: info, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Where a constraint's glyph goes (sketch coordinates): beside the middle of its first curve,
/// or at its point.
fn anchor(sk: &Sketch, k: &ConstraintKind) -> Option<Vec2> {
    use ConstraintKind::*;
    // Half way along the curve.
    let mid = |c: usize| -> Option<Vec2> {
        let poly = sk.polyline(c);
        let total: f64 = poly.windows(2).map(|w| w[0].dist(w[1])).sum();
        let mut acc = 0.0;
        for w in poly.windows(2) {
            let l = w[0].dist(w[1]);
            if acc + l >= total * 0.5 && l > 0.0 {
                return Some(w[0].lerp(w[1], (total * 0.5 - acc) / l));
            }
            acc += l;
        }
        poly.first().copied()
    };
    let pt = |p: usize| sk.point(p);
    match *k {
        OffsetDistance { ref output, .. } => sk.offset_dimension_points(output).map(|(a, b)| (a + b) * 0.5),
        PatternPoint { q, center, .. } => pt(center.unwrap_or(q)),
        PatternRadius { b, .. } => mid(b),
        Coincident { p, .. } | Fix { p } | Midpoint { p, .. } => pt(p),
        // A pierce is drawn at the point it holds.
        Pierce { p, .. } => pt(p),
        PointOnCurve { p, .. } | PointAlong { p, .. } => pt(p),
        HorizontalPoints { p, q } | VerticalPoints { p, q } | Symmetric { p, q, .. } => Some((pt(p)? + pt(q)?) * 0.5),
        Horizontal { l } | Vertical { l } => mid(l),
        Parallel { a, .. } | Perpendicular { a, .. } | Collinear { a, .. } | Equal { a, .. } | Concentric { a, .. } | Angle { a, .. } => mid(a),
        Tangent { a, b } | Smooth { a, b } => {
            // At the shared end when there is one.
            let ends = |c: usize| sk.curves.get(c).and_then(|c| c.kind.ends());
            match (ends(a), ends(b)) {
                (Some((a0, a1)), Some((b0, b1))) => [a0, a1].into_iter().find(|x| *x == b0 || *x == b1).and_then(pt).or_else(|| mid(a)),
                _ => mid(a),
            }
        }
        Distance { p, q, .. } | DistanceX { p, q, .. } | DistanceY { p, q, .. } => Some((pt(p)? + pt(q)?) * 0.5),
        PointLineDistance { p, .. } | LinearDiameter { p, .. } => pt(p),
        LineCircleDistance { c, .. } => sk.center(c),
        PointCircleDistance { p, .. } => pt(p),
        CircleCircleDistance { a, b, .. } => Some((sk.center(a)? + sk.center(b)?) * 0.5),
        Length { l, .. } | LineAngle { l, .. } => mid(l),
        Radius { c, .. } | Diameter { c, .. } | ArcLength { c, .. } | ArcSweep { c, .. } => mid(c),
        SlotOverallLength { a, b, .. } => Some((sk.center(a)? + sk.center(b)?) * 0.5),
    }
}

fn glyphs(s: &mut Session, p: &Value) -> Result<Value> {
    let id = sketch_store::target(s, p, "sketch.glyphs")?;
    let st = s.model.state();
    let ss = st.sketch(id).ok_or_else(|| EngineError::Other("that sketch is not evaluated".into()))?;
    let failing = &ss.report.failing;
    let glyphs: Vec<Glyph> = ss
        .sketch
        .constraints
        .iter()
        .map(|c| Glyph {
            id: c.id.clone(),
            name: c.kind.name().to_string(),
            dimension: c.kind.is_dimension(),
            driven: c.driven,
            value: c.kind.value(),
            param: c.param.clone(),
            at: anchor(&ss.sketch, &c.kind).map(|a| [a.x, a.y]),
            failing: failing.contains(&c.id),
            // Implied by the others (it removes no freedom) but holding: shown dimmed.
            redundant: ss.report.redundant.contains(&c.id),
        })
        .collect();
    let result = GlyphsResult {
        sketch: id,
        glyphs,
        dof: ss.report.dof,
        fully_constrained: ss.report.fully_constrained(),
        conflicts: failing.clone(),
        param_warnings: Vec::new(),
    };
    reply("sketch.glyphs", result)
}

/// Snap inference for a click at `at`: the nearest sketch point (coincident), the midpoint of a
/// line, a point on a curve, and horizontal/vertical alignment with `from` or with other
/// points. Results are ordered by priority; the first one is what a click would use.
fn snap(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.snap";
    let at = req_vec2(cmd, p, "at")?;
    let from = p.get("from").and_then(vec2);
    let r = num(p, "radius").filter(|r| *r > 0.0).unwrap_or(1.0);
    let id = sketch_store::active(s).ok_or_else(|| bad(cmd, "no sketch is being edited"))?;
    let st = s.model.state();
    let ss = st.sketch(id).ok_or_else(|| EngineError::Other("the sketch is not evaluated".into()))?;
    let sk = &ss.sketch;
    let mut out: Vec<(f64, Snap)> = Vec::new();
    // Points (end points, centres, the origin).
    for q in &sk.points {
        let d = q.pos.dist(at);
        if d <= r {
            out.push((d * 0.5, Snap::Coincident { point: q.id.clone(), at: [q.pos.x, q.pos.y] }));
        }
    }
    for (i, c) in sk.curves.iter().enumerate() {
        if let CurveKind::Line { a, b } = c.kind
            && let (Some(pa), Some(pb)) = (sk.point(a), sk.point(b))
        {
            let m = (pa + pb) * 0.5;
            let d = m.dist(at);
            if d <= r {
                out.push((d * 0.6, Snap::Midpoint { curve: c.id.clone(), at: [m.x, m.y] }));
            }
        }
        if let Some(sh) = sk.shape(i) {
            let q = sh.project(at);
            if sh.dist(at) <= r && q.dist(at) <= r {
                out.push((sh.dist(at) + r * 0.5, Snap::OnCurve { curve: c.id.clone(), at: [q.x, q.y] }));
            }
        }
    }
    // Alignment with the previous click.
    if let Some(f) = from {
        let d = at - f;
        if d.y.abs() <= r && d.x.abs() > r {
            out.push((d.y.abs() + r, Snap::Horizontal { at: [at.x, f.y] }));
        }
        if d.x.abs() <= r && d.y.abs() > r {
            out.push((d.x.abs() + r, Snap::Vertical { at: [f.x, at.y] }));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    let snaps: Vec<Snap> = out.into_iter().take(8).map(|x| x.1).collect();
    reply(cmd, SnapResult { snaps, param_warnings: Vec::new() })
}

#[cfg(test)]
#[path = "sketch_constraints_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sketch_fuzz_tests.rs"]
mod fuzz;

#[cfg(test)]
#[path = "sketch_perf_tests.rs"]
mod perf;
