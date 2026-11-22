//! The inspection plan (`inspect.check_*`): checks picked on the model, each with its tolerance,
//! kept in the design (`Document::checks`) and measured on a scan aligned to the model.
//!
//! A check's nominal comes from the model's exact faces: a round face's own cylinder, a flat face's
//! own plane and its outline from its edges' exact curves. The face a check names is found again
//! by its body and point, as a pick is (`face_index_at`). Its actual comes from the scan points on
//! that face, fitted robustly (`solidcraft_scan::inspect_plan`). Tolerances not given come from a
//! general-tolerance class by size.

use std::sync::Arc;

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_doc::inspection::{Check, CheckKind, Class, MAX_CHECKS};
use solidcraft_geom::prelude::*;
use solidcraft_scan::inspect_plan::{self, Cylinder};

use super::CommandSpec;
use crate::params::{bad, bool_, str_};
use crate::schema::inspection_plan::{
    CheckAddParams, CheckAddResult, CheckItem, CheckListParams, CheckListResult, CheckMeasureParams, CheckMeasureResult, CheckMeasured,
    CheckRemoveParams, CheckRemoveResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const ADD: TypedContract<CheckAddParams> = TypedContract::new();
const REMOVE: TypedContract<CheckRemoveParams> = TypedContract::new();
const LIST: TypedContract<CheckListParams> = TypedContract::new();
const MEASURE: TypedContract<CheckMeasureParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("inspect.check_add", "Add Check", crate::checked!("inspect.check_add", check_add))
        .at("INSPECT", "MEASURE")
        .icon("measure")
        .contract(&ADD)
        .output(output_schema_for::<CheckAddResult>)
        .params("kind: diameter|distance|flatness|position|point|auto (a round face's diameter, a flat face's flatness); face: {body, point} (a point on the face picked); face2?: {body, point} (a distance's second, parallel flat face); tolerance?: length (± about the nominal, or the zone for flatness and position) | plus?, minus?: lengths; class?: fine|medium|coarse (default medium: the general tolerance for the size when none is given); name?: a check added to the design's inspection plan, its nominal read from the model's exact face, one undo step → check, kind, nominal, plus, minus"),
    CommandSpec::new("inspect.check_remove", "Remove Check", crate::checked!("inspect.check_remove", check_remove))
        .icon("delete")
        .contract(&REMOVE)
        .output(output_schema_for::<CheckRemoveResult>)
        .params("check: id | all: true: taken out of the inspection plan, one undo step → removed, left"),
    CommandSpec::new("inspect.check_list", "List Checks", crate::checked!("inspect.check_list", check_list))
        .icon("list")
        .noundo()
        .read_only()
        .contract(&LIST)
        .output(output_schema_for::<CheckListResult>)
        .params("→ checks: [{check, kind, name, body, at, at2, nominal, plus, minus, problem}] in the order added"),
    CommandSpec::new("inspect.check_measure", "Measure Checks", crate::checked!("inspect.check_measure", check_measure))
        .at("INSPECT", "MEASURE")
        .icon("measure")
        .noundo()
        .contract(&MEASURE)
        .output(output_schema_for::<CheckMeasureResult>)
        .params("scan?: mesh body (default: the selected or only one); band?: length (how far off the model's face a scan point is still read for it; default four tolerances, at least 0.5 mm); html?, csv?: paths (a report written there); show?: bool (false takes the results out of the view): every check of the plan measured on the scan (aligned to the model first), shown in the view, a view and not an undo step → checks [{…, actual, deviation, pass, points}], passed, failed, unmeasured"),
];

/// The results `inspect.check_measure` last showed, for the viewport to label. View state: not
/// part of the design and not an undo step; it stands for the design as it was at `revision`.
#[derive(Clone, Debug, PartialEq)]
pub struct Inspection {
    pub revision: u64,
    pub results: Vec<ShownCheck>,
}

/// A check as the view labels it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownCheck {
    pub number: usize,
    pub at: Vec3,
    pub kind: CheckKind,
    pub nominal: Option<f64>,
    pub actual: Option<f64>,
    pub pass: Option<bool>,
}

impl Inspection {
    pub fn current(&self, s: &Session) -> bool {
        self.revision == s.revision
    }
}

/// What a check reads on the model.
enum Nominal {
    /// A round face's own cylinder.
    Round { cyl: solidcraft_kernel::CylinderFace },
    /// A flat face: its plane (outward normal) and outline.
    Flat { point: Vec3, normal: Vec3, outline: Vec<Vec<Vec3>> },
    /// Two parallel flat faces.
    Pair { a: (Vec3, Vec3, Vec<Vec<Vec3>>), b: (Vec3, Vec3, Vec<Vec<Vec3>>) },
    /// A point on a face and the face's outward normal there.
    At { point: Vec3, normal: Vec3 },
}

impl Nominal {
    /// The model's value: a diameter, a distance, or 0 (flatness, position, a point's offset).
    fn value(&self, kind: CheckKind) -> f64 {
        match (self, kind) {
            (Nominal::Round { cyl }, CheckKind::Diameter) => 2.0 * cyl.radius,
            (Nominal::Pair { a, b }, _) => (b.0 - a.0).dot(a.1).abs(),
            _ => 0.0,
        }
    }
}

/// The face of `body` at `at`, read exactly for `kind`.
fn face_of(s: &Session, body: &str, at: Vec3) -> std::result::Result<(usize, f64), String> {
    let state = s.model.state();
    let b = state.body(body).ok_or_else(|| format!("no body `{body}`"))?;
    if b.body.is_mesh() {
        return Err(format!("`{body}` is a mesh body: pick a face of the model"));
    }
    let face = solidcraft_doc::appearance::face_index_at(b, at).ok_or_else(|| format!("no face of `{body}` at that point"))?;
    Ok((face, b.body.size()))
}

fn flat(s: &Session, body: &str, at: Vec3) -> std::result::Result<(Vec3, Vec3, Vec<Vec<Vec3>>), String> {
    let (face, _) = face_of(s, body, at)?;
    let state = s.model.state();
    let b = state.body(body).ok_or_else(|| format!("no body `{body}`"))?;
    match solidcraft_kernel::face_surface(&b.body, face) {
        Some(solidcraft_kernel::FaceSurface::Plane { normal, point }) => Ok((point, *normal, solidcraft_kernel::face_outline(&b.body, face))),
        _ => Err("the face there is not flat".into()),
    }
}

fn round(s: &Session, body: &str, at: Vec3) -> std::result::Result<solidcraft_kernel::CylinderFace, String> {
    let (face, _) = face_of(s, body, at)?;
    let state = s.model.state();
    let b = state.body(body).ok_or_else(|| format!("no body `{body}`"))?;
    solidcraft_kernel::cylinder_face_exact(&b.body, face).ok_or_else(|| "the face there is not round (a cylinder)".into())
}

/// What check `c` reads on the model now.
fn nominal(s: &Session, c: &Check) -> std::result::Result<Nominal, String> {
    match c.kind {
        CheckKind::Diameter | CheckKind::Position => Ok(Nominal::Round { cyl: round(s, &c.body, c.at)? }),
        CheckKind::Flatness => {
            let (point, normal, outline) = flat(s, &c.body, c.at)?;
            Ok(Nominal::Flat { point, normal, outline })
        }
        CheckKind::Distance => {
            let a = flat(s, &c.body, c.at)?;
            let b = flat(s, &c.body, c.at2.ok_or("a distance needs its second face")?)?;
            if a.1.dot(b.1).abs() < 1.0 - 1e-6 {
                return Err("the two faces are not parallel".into());
            }
            Ok(Nominal::Pair { a, b })
        }
        CheckKind::Point => {
            let (face, _) = face_of(s, &c.body, c.at)?;
            let state = s.model.state();
            let b = state.body(&c.body).ok_or_else(|| format!("no body `{}`", c.body))?;
            let normal = match solidcraft_kernel::face_surface(&b.body, face) {
                Some(solidcraft_kernel::FaceSurface::Plane { normal, .. }) => *normal,
                Some(solidcraft_kernel::FaceSurface::Cylinder { .. }) => {
                    let cyl = solidcraft_kernel::cylinder_face_exact(&b.body, face).ok_or("the round face cannot be read")?;
                    let d = c.at - cyl.axis_point;
                    let radial = (d - *cyl.axis * d.dot(*cyl.axis)).normalized().ok_or("the point is on the face's axis")?;
                    if cyl.internal { -radial } else { radial }
                }
                _ => return Err("a point check reads a flat or a round face".into()),
            };
            Ok(Nominal::At { point: c.at, normal })
        }
    }
}

/// The largest extent of a face's outline: what its flatness tolerance is read by.
fn extent(outline: &[Vec<Vec3>]) -> f64 {
    let pts: Vec<Vec3> = outline.iter().flatten().copied().collect();
    let mut best = 0.0f64;
    for (i, a) in pts.iter().enumerate() {
        for b in pts.iter().skip(i + 1) {
            best = best.max(a.dist(*b));
        }
    }
    best
}

fn kind_of(s: &str) -> Option<CheckKind> {
    Some(match s {
        "diameter" => CheckKind::Diameter,
        "distance" => CheckKind::Distance,
        "flatness" => CheckKind::Flatness,
        "position" => CheckKind::Position,
        "point" => CheckKind::Point,
        _ => return None,
    })
}

fn length(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<Option<f64>> {
    let Some(e) = crate::params::expr(p, key) else { return Ok(None) };
    let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("{key}: {e}")))?;
    if !(v.is_finite() && v >= 0.0) {
        return Err(bad(cmd, format!("`{key}` must be a length of 0 or more")));
    }
    Ok(Some(v))
}

fn face_param(p: &Value, key: &str, cmd: &str) -> Result<Option<(String, Vec3)>> {
    let Some(f) = p.get(key).filter(|v| !v.is_null()) else { return Ok(None) };
    let body = f.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, format!("`{key}.body` is a body name")))?;
    let point = f
        .get("point")
        .and_then(crate::params::vec3)
        .filter(|q| q.is_finite())
        .ok_or_else(|| bad(cmd, format!("`{key}.point` is a point [x, y, z]")))?;
    Ok(Some((body.to_owned(), point)))
}

fn item(s: &Session, c: &Check) -> CheckItem {
    let found = nominal(s, c);
    CheckItem {
        check: c.id,
        kind: c.kind.name().into(),
        name: c.name.clone(),
        body: c.body.clone(),
        at: [c.at.x, c.at.y, c.at.z],
        at2: c.at2.map(|q| [q.x, q.y, q.z]),
        nominal: found.as_ref().ok().map(|n| n.value(c.kind)),
        plus: c.plus,
        minus: c.minus,
        problem: found.err(),
    }
}

fn check_add(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.check_add";
    let (body, at) = face_param(p, "face", cmd)?.ok_or_else(|| bad(cmd, "`face` is {body, point} on the face"))?;
    let kind = match str_(p, "kind") {
        // What the face is decides: a round face's diameter, a flat face's flatness.
        Some("auto") => match (round(s, &body, at), flat(s, &body, at)) {
            (Ok(_), _) => CheckKind::Diameter,
            (_, Ok(_)) => CheckKind::Flatness,
            (Err(e), _) => return Err(bad(cmd, format!("{e}; `auto` reads a round or a flat face"))),
        },
        k => k.and_then(kind_of).ok_or_else(|| bad(cmd, "`kind` is diameter, distance, flatness, position, point or auto"))?,
    };
    let second = face_param(p, "face2", cmd)?;
    let at2 = match (kind, second) {
        (CheckKind::Distance, Some((b2, q))) if b2 == body => Some(q),
        (CheckKind::Distance, Some(_)) => return Err(bad(cmd, "a distance's two faces are on one body")),
        (CheckKind::Distance, None) => return Err(bad(cmd, "a distance needs `face2`")),
        (_, Some(_)) => return Err(bad(cmd, "only a distance takes `face2`")),
        (_, None) => None,
    };
    if s.doc.checks.len() >= MAX_CHECKS {
        return Err(bad(cmd, format!("a plan holds at most {MAX_CHECKS} checks")));
    }
    let class = match str_(p, "class") {
        Some(c) => Class::parse(c).ok_or_else(|| bad(cmd, "`class` is fine, medium or coarse"))?,
        None => Class::Medium,
    };
    let name = str_(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    let draft = Check { id: 0, kind, body, at, at2, plus: 1.0, minus: 1.0, name };
    let found = nominal(s, &draft).map_err(|e| bad(cmd, e))?;
    let value = found.value(kind);
    let (tol, plus, minus) = (length(s, p, "tolerance", cmd)?, length(s, p, "plus", cmd)?, length(s, p, "minus", cmd)?);
    if tol.is_some() && (plus.is_some() || minus.is_some()) {
        return Err(bad(cmd, "give `tolerance`, or `plus` and `minus`, not both"));
    }
    if kind.zone() && (plus.is_some() || minus.is_some()) {
        return Err(bad(cmd, "flatness and position take a `tolerance` (a zone), not `plus` and `minus`"));
    }
    let general = match (&found, kind) {
        (Nominal::Flat { outline, .. }, CheckKind::Flatness) => class.flatness(extent(outline)),
        (Nominal::Round { cyl }, CheckKind::Position) => 2.0 * class.linear(2.0 * cyl.radius),
        (_, CheckKind::Point) => {
            let size = s.model.state().body(&draft.body).map_or(0.0, |b| b.body.size());
            class.linear(size)
        }
        _ => class.linear(value),
    };
    let (plus, minus) = match (tol, plus, minus) {
        (Some(t), _, _) if kind.zone() => (t, 0.0),
        (Some(t), _, _) => (t, t),
        (None, None, None) if kind.zone() => (general, 0.0),
        (None, None, None) => (general, general),
        (None, p, m) => (p.unwrap_or(0.0), m.unwrap_or(0.0)),
    };
    let id = s.doc_mut().allocate_id()?;
    let check = Check { id, plus, minus, ..draft };
    if !check.is_valid() {
        return Err(bad(cmd, "the tolerance must be above 0"));
    }
    let shown = item(s, &check);
    s.doc_mut().checks.push(check);
    serde_json::to_value(CheckAddResult { item: shown, param_warnings: Vec::new() }).map_err(|e| EngineError::Other(e.to_string()))
}

fn check_remove(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.check_remove";
    let removed: Vec<u64> = match (p.get("check").and_then(Value::as_u64), bool_(p, "all") == Some(true)) {
        (Some(_), true) | (None, false) => return Err(bad(cmd, "give `check` (an id) or `all: true`")),
        (None, true) => std::mem::take(&mut s.doc_mut().checks).into_iter().map(|c| c.id).collect(),
        (Some(id), false) => {
            let i = s.doc.checks.iter().position(|c| c.id == id).ok_or_else(|| bad(cmd, format!("no check {id}")))?;
            vec![s.doc_mut().checks.remove(i).id]
        }
    };
    let left = s.doc.checks.len();
    serde_json::to_value(CheckRemoveResult { removed, left, param_warnings: Vec::new() }).map_err(|e| EngineError::Other(e.to_string()))
}

fn check_list(s: &mut Session, _p: &Value) -> Result<Value> {
    let checks = s.doc.checks.iter().map(|c| item(s, c)).collect();
    serde_json::to_value(CheckListResult { checks, param_warnings: Vec::new() }).map_err(|e| EngineError::Other(e.to_string()))
}

/// Check `c` measured on the scan `points`: the actual value and how many points it read.
fn measure_one(points: &[Vec3], c: &Check, found: &Nominal, band: f64) -> Option<(f64, usize)> {
    let cyl = |k: &solidcraft_kernel::CylinderFace| Cylinder { point: k.axis_point, axis: *k.axis, radius: k.radius, start: k.start, end: k.end };
    match (found, c.kind) {
        (Nominal::Round { cyl: k }, CheckKind::Diameter) => inspect_plan::fit_cylinder(points, &cyl(k), band).map(|f| (2.0 * f.radius, f.points)),
        (Nominal::Round { cyl: k }, CheckKind::Position) => {
            inspect_plan::fit_cylinder(points, &cyl(k), band).map(|f| (2.0 * f.centre[0].hypot(f.centre[1]), f.points))
        }
        (Nominal::Flat { point, normal, outline }, CheckKind::Flatness) => {
            inspect_plan::fit_plane(points, *point, *normal, band, |q| inspect_plan::inside_outline(q, *normal, outline, band))
                .map(|f| (f.spread, f.points))
        }
        (Nominal::Pair { a, b }, CheckKind::Distance) => {
            let fa = inspect_plan::fit_plane(points, a.0, a.1, band, |q| inspect_plan::inside_outline(q, a.1, &a.2, band))?;
            let fb = inspect_plan::fit_plane(points, b.0, b.1, band, |q| inspect_plan::inside_outline(q, b.1, &b.2, band))?;
            Some(((fb.point - fa.point).dot(a.1).abs(), fa.points + fb.points))
        }
        (Nominal::At { point, normal }, CheckKind::Point) => inspect_plan::offset_at(points, *point, *normal, band.max(1.0), band),
        _ => None,
    }
}

fn check_measure(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.check_measure";
    let empty = |shown| CheckMeasureResult {
        shown,
        scan: None,
        checks: Vec::new(),
        passed: 0,
        failed: 0,
        unmeasured: 0,
        html: None,
        csv: None,
        param_warnings: Vec::new(),
    };
    let to_value = |r: CheckMeasureResult| serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()));
    if bool_(p, "show") == Some(false) {
        s.inspection = None;
        return to_value(empty(false));
    }
    if s.doc.checks.is_empty() {
        return Err(bad(cmd, "the inspection plan has no checks: add one with inspect.check_add"));
    }
    let scan = super::scan::mesh_body_name(s, str_(p, "scan"), cmd)?;
    let band = length(s, p, "band", cmd)?;
    let points: Vec<Vec3> = {
        let state = s.model.state();
        let (positions, _) = state.body(&scan).and_then(|b| b.body.triangle_mesh()).ok_or_else(|| bad(cmd, format!("`{scan}` is not a scan")))?;
        positions.to_vec()
    };
    let mut rows = Vec::with_capacity(s.doc.checks.len());
    for c in &s.doc.checks {
        let it = item(s, c);
        let measured = nominal(s, c).ok().and_then(|n| {
            let band = band.unwrap_or_else(|| (4.0 * (c.plus + c.minus)).max(0.5));
            measure_one(&points, c, &n, band)
        });
        let (actual, count) = measured.map_or((None, 0), |(a, n)| (Some(a), n));
        let deviation = actual.zip(it.nominal).map(|(a, n)| a - n);
        let pass = actual.zip(it.nominal).map(|(a, n)| if c.kind.zone() { a <= c.plus } else { a >= n - c.minus && a <= n + c.plus });
        rows.push(CheckMeasured { item: it, actual, deviation, pass, points: count });
    }
    let passed = rows.iter().filter(|r| r.pass == Some(true)).count();
    let failed = rows.iter().filter(|r| r.pass == Some(false)).count();
    let unmeasured = rows.len() - passed - failed;
    let html = str_(p, "html").map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned);
    let csv = str_(p, "csv").map(str::trim).filter(|x| !x.is_empty()).map(str::to_owned);
    if let Some(path) = &html {
        std::fs::write(path, html_report(&s.doc.name, &scan, &rows)).map_err(|e| bad(cmd, format!("could not write `{path}`: {e}")))?;
    }
    if let Some(path) = &csv {
        std::fs::write(path, csv_report(&rows)).map_err(|e| bad(cmd, format!("could not write `{path}`: {e}")))?;
    }
    s.inspection = Some(Arc::new(Inspection {
        revision: s.revision,
        results: s
            .doc
            .checks
            .iter()
            .zip(&rows)
            .enumerate()
            .map(|(i, (c, r))| ShownCheck { number: i + 1, at: c.at, kind: c.kind, nominal: r.item.nominal, actual: r.actual, pass: r.pass })
            .collect(),
    }));
    to_value(CheckMeasureResult { scan: Some(scan), checks: rows, passed, failed, unmeasured, html, csv, ..empty(true) })
}

fn verdict(r: &CheckMeasured) -> &'static str {
    match r.pass {
        Some(true) => "pass",
        Some(false) => "fail",
        None => "not measured",
    }
}

fn mm(v: Option<f64>) -> String {
    v.map_or_else(String::new, |v| format!("{v:.3}"))
}

fn csv_report(rows: &[CheckMeasured]) -> String {
    let quote = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let mut out = String::from("number,check,name,kind,body,nominal,plus,minus,actual,deviation,result,points\n");
    for (i, r) in rows.iter().enumerate() {
        out += &format!(
            "{},{},{},{},{},{},{:.3},{:.3},{},{},{},{}\n",
            i + 1,
            r.item.check,
            quote(r.item.name.as_deref().unwrap_or("")),
            r.item.kind,
            quote(&r.item.body),
            mm(r.item.nominal),
            r.item.plus,
            r.item.minus,
            mm(r.actual),
            mm(r.deviation),
            verdict(r),
            r.points
        );
    }
    out
}

fn html_report(design: &str, scan: &str, rows: &[CheckMeasured]) -> String {
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let passed = rows.iter().filter(|r| r.pass == Some(true)).count();
    let mut out = format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Inspection report: {d}</title>\n<style>body{{font-family:sans-serif;margin:2em}}table{{border-collapse:collapse}}td,th{{border:1px solid #999;padding:4px 8px;text-align:right}}td.l,th.l{{text-align:left}}.fail{{background:#fdd}}.pass{{background:#dfd}}</style></head><body>\n<h1>Inspection report</h1>\n<p>Design: {d}<br>Scan: {s}<br>{passed} of {n} checks pass.</p>\n<table><tr><th>#</th><th class=\"l\">Check</th><th class=\"l\">Kind</th><th>Nominal</th><th>+</th><th>−</th><th>Actual</th><th>Deviation</th><th class=\"l\">Result</th><th>Points</th></tr>\n",
        d = esc(design),
        s = esc(scan),
        n = rows.len()
    );
    for (i, r) in rows.iter().enumerate() {
        let class = match r.pass {
            Some(true) => "pass",
            Some(false) => "fail",
            None => "",
        };
        out += &format!(
            "<tr class=\"{class}\"><td>{}</td><td class=\"l\">{}</td><td class=\"l\">{}</td><td>{}</td><td>{:.3}</td><td>{:.3}</td><td>{}</td><td>{}</td><td class=\"l\">{}</td><td>{}</td></tr>\n",
            i + 1,
            esc(r.item.name.as_deref().unwrap_or("")),
            r.item.kind,
            mm(r.item.nominal),
            r.item.plus,
            r.item.minus,
            mm(r.actual),
            mm(r.deviation),
            verdict(r),
            r.points
        );
    }
    out + "</table>\n<p>Values in mm. Measured from the scan points on each face of the model, least-squares fits.</p>\n</body></html>\n"
}

#[cfg(test)]
#[path = "inspection_plan_tests.rs"]
mod tests;
