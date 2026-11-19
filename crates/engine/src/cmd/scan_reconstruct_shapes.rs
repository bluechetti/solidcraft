//! Loft It, Pipe It and Coil It: the parts Reconstruct reads that are no prism and no turned
//! part. Loft It sketches each section the scan's slices give as a closed fit-point spline and
//! lofts them; Pipe It sketches the path through a bent bar's axes (lines and arcs, held
//! tangent where they run on into each other) and sweeps the bar's circle along it; Coil It
//! sketches the wire's section where it starts and winds it with Coil.

use serde_json::{Value, json};
use solidcore_record::{Plane, Vec2, Vec3};
use solidcraft_geom::prelude::*;
use solidcraft_scan::pipe::Piece;

use super::{View, at, into_params, merged, reply, run, view};
use crate::params::bad;
use crate::schema::scan_reconstruct::{ScanBuildResult, ScanCoil, ScanLoft, ScanLoftStation, ScanPipe};
use crate::{EngineError, Result, Session};

fn in_plane(plane: &Plane, p: Vec3) -> [f64; 2] {
    let d = p - plane.origin;
    [d.dot(plane.x), d.dot(plane.y)]
}

/// A new sketch on `plane`, left open for drawing; its id.
fn open_sketch(s: &mut Session, plane: &Plane, name: &str) -> Result<u64> {
    let made = super::super::new_sketch(s, json!({"plane": {"origin": at(plane.origin), "x_dir": at(plane.x), "y_dir": at(plane.y)}, "name": name}))?;
    made.get("sketch").and_then(Value::as_u64).ok_or_else(|| EngineError::Other("the sketch was not made".into()))
}

/// The curve ids a draw command's reply lists.
fn curves(reply: &Value) -> Vec<String> {
    reply.get("curves").and_then(Value::as_array).map(|c| c.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()).unwrap_or_default()
}

/// Runs `draw` in the open sketch, cancelling the sketch when it fails.
fn drawing<T>(s: &mut Session, draw: impl FnOnce(&mut Session) -> Result<T>) -> Result<T> {
    match draw(s) {
        Ok(done) => {
            // Dimensioned from its origin, as a profile drawn by hand is.
            super::super::section::finish_dimensioned(s, true, None)?;
            Ok(done)
        }
        Err(e) => {
            let _ = run(s, "sketch.cancel", json!({}));
            Err(e)
        }
    }
}

fn built(view: &View, sketch: Option<u64>, feature: Value, solid: String, how: &str, depth: Option<f64>, lines: usize, arcs: usize) -> Result<Value> {
    reply(ScanBuildResult {
        body: view.found.scan.clone(),
        sketch,
        feature,
        solid,
        operation: how.into(),
        lines,
        arcs,
        tangents: 0,
        depth,
        count: None,
        openings: None,
        skin: None,
        allowance: None,
        fit: None,
        param_warnings: Vec::new(),
    })
}

/// The reading's coil as the reply lists it.
pub(super) fn coil_of(c: &solidcraft_scan::coil::Coil) -> ScanCoil {
    ScanCoil {
        base: at(c.base),
        axis: at(c.axis),
        diameter: c.diameter,
        wire: c.wire,
        pitch: c.pitch,
        turns: c.turns,
        clockwise: c.clockwise,
        faces: c.faces.iter().copied().collect(),
    }
}

fn piece_length(p: &Piece, normal: Vec3) -> f64 {
    match *p {
        Piece::Line { from, to } => from.dist(to),
        Piece::Arc { centre, from, to, clockwise } => {
            let (a, b) = (from - centre, to - centre);
            let mut turn = a.cross(b).dot(normal).atan2(a.dot(b));
            if clockwise {
                turn = -turn;
            }
            if turn < 0.0 {
                turn += std::f64::consts::TAU;
            }
            a.len() * turn
        }
    }
}

/// The reading's pipe as the reply lists it.
pub(super) fn pipe_of(p: &solidcraft_scan::pipe::Pipe) -> ScanPipe {
    ScanPipe {
        diameter: 2.0 * p.radius,
        lines: p.pieces.iter().filter(|k| matches!(k, Piece::Line { .. })).count(),
        arcs: p.pieces.iter().filter(|k| matches!(k, Piece::Arc { .. })).count(),
        length: p.pieces.iter().map(|k| piece_length(k, p.normal)).sum(),
        faces: p.faces.iter().copied().collect(),
    }
}

/// The reading's loft as the reply lists it.
pub(super) fn loft_of(l: &solidcraft_scan::loft::Loft) -> ScanLoft {
    let length = match (l.sections.first(), l.sections.last()) {
        (Some(a), Some(b)) => (b.plane.origin - a.plane.origin).dot(l.direction).abs(),
        _ => 0.0,
    };
    let first = l.sections.first().map(|s| s.plane.origin);
    let base = l.sections.iter().find_map(|s| s.shape).map(|f| f.twist);
    let stations = l
        .sections
        .iter()
        .filter_map(|section| {
            let found = section.shape?;
            let plane = &section.plane;
            Some(ScanLoftStation {
                at: first.map_or(0.0, |o| (plane.origin - o).dot(l.direction).abs()),
                chord: found.chord,
                twist: (found.twist - base.unwrap_or(found.twist)).to_degrees(),
                thickness: found.thickness,
                thickness_share: found.thickness / found.chord,
                thickest_at: found.thickest_at,
                camber_share: found.camber / found.chord,
                leading: at(plane.origin + plane.x * found.leading.x + plane.y * found.leading.y),
                naca: found.naca.map(|[m, p, t]| format!("{m}{p}{t:02}")),
            })
        })
        .collect();
    ScanLoft { direction: at(l.direction), capped: l.capped, sections: l.sections.len(), length, stations }
}

/// Loft It: each section sketched as a closed spline through its points, lofted in order.
pub(super) fn loft_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.loft_it";
    let view = view(s, p, cmd, false)?;
    let loft =
        view.reading().loft.clone().ok_or_else(|| {
            bad(cmd, "Reconstruct reads no lofted part: the part is read as prisms or turned, or a slice of it is not one closed loop")
        })?;
    let (operation, solid, how) = into_params(s, &view.found.scan, p, cmd)?;
    let mut sketches = Vec::new();
    for (k, section) in loft.sections.iter().enumerate() {
        let id = open_sketch(s, &section.plane, &format!("Loft Section {}", k + 1))?;
        let points: Vec<[f64; 2]> = section.points.iter().map(|q: &Vec2| [q.x, q.y]).collect();
        // The spline's points are held where the scan has them, not each given two sizes of its
        // own (72 to a section, none of them a number of the design): what a section measures is
        // in Reconstruct's `loft.stations`.
        if let Err(e) = run(s, "sketch.spline.fit_point", json!({"points": points, "periodic": true})) {
            let _ = run(s, "sketch.cancel", json!({}));
            return Err(e);
        }
        super::super::super::sketch_store::update(s, id, |_, _, sk| {
            for point in 1..sk.points.len() {
                sk.add_constraint(solidcraft_sketch::ConstraintKind::Fix { p: point }, None)?;
            }
            Ok(())
        })?;
        run(s, "sketch.finish", json!({}))?;
        sketches.push(json!({"sketch": id}));
    }
    let feature = run(s, "solid.loft", merged(json!({"sections": sketches, "name": "Loft It"}), operation))
        .map_err(|e| super::super::refusal::refused(cmd, "the kernel could not loft the sections the scan gives", &e.to_string()))?;
    let length = loft_of(&loft).length;
    built(&view, None, feature, solid, how, Some(length), 0, 0)
}

/// Pipe It: the path sketched (lines and arcs in the path's plane) and the bar swept along it.
pub(super) fn pipe_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.pipe_it";
    let view = view(s, p, cmd, false)?;
    let pipe = view.reading().pipe.clone().ok_or_else(|| bad(cmd, "Reconstruct reads no bar bent along a flat path"))?;
    let (operation, solid, how) = into_params(s, &view.found.scan, p, cmd)?;
    let (x, y) = {
        let first = pipe.pieces.first().map(|k| k.ends()).ok_or_else(|| bad(cmd, "the path has no piece"))?;
        let x = (first.1 - first.0).normalized().or_else(|| pipe.normal.any_perp().normalized()).unwrap_or(Vec3::X);
        (x, pipe.normal.cross(x))
    };
    let plane = Plane { origin: pipe.origin, x, y };
    let sketch = open_sketch(s, &plane, "Pipe Path")?;
    let path = drawing(s, |s| {
        let mut ids = Vec::new();
        for piece in &pipe.pieces {
            let drawn = match *piece {
                Piece::Line { from, to } => run(s, "sketch.line", json!({"points": [in_plane(&plane, from), in_plane(&plane, to)]}))?,
                Piece::Arc { centre, from, to, clockwise } => run(
                    s,
                    "sketch.arc.center_point",
                    json!({"center": in_plane(&plane, centre), "start": in_plane(&plane, from), "end": in_plane(&plane, to), "clockwise": clockwise}),
                )?,
            };
            ids.extend(curves(&drawn));
        }
        Ok(ids)
    })?;
    let lines = pipe.pieces.iter().filter(|k| matches!(k, Piece::Line { .. })).count();
    let feature = run(
        s,
        "solid.pipe",
        merged(json!({"path_sketch": sketch, "path": path, "diameter": format!("{} mm", 2.0 * pipe.radius), "name": "Pipe It"}), operation),
    )
    .map_err(|e| super::super::refusal::refused(cmd, "the kernel could not sweep the bar along the path the scan gives", &e.to_string()))?;
    built(&view, Some(sketch), feature, solid, how, Some(pipe_of(&pipe).length), lines, pipe.pieces.len() - lines)
}

/// Coil It: the wire's section sketched where it starts, on the plane through the axis, and
/// wound with Coil at the pitch and turns read.
pub(super) fn coil_it(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.coil_it";
    let view = view(s, p, cmd, false)?;
    let coil = view.reading().coil.clone().ok_or_else(|| bad(cmd, "Reconstruct reads no coil spring"))?;
    let (operation, solid, how) = into_params(s, &view.found.scan, p, cmd)?;
    let plane = Plane { origin: coil.base, x: coil.start, y: coil.axis };
    let sketch = open_sketch(s, &plane, "Coil Section")?;
    drawing(s, |s| run(s, "sketch.circle.center", json!({"center": [0.5 * coil.diameter, 0.0], "radius": 0.5 * coil.wire})))?;
    let feature = run(
        s,
        "solid.coil",
        merged(
            json!({"sketch": sketch, "base": at(coil.base), "axis": at(coil.axis), "pitch": format!("{} mm", coil.pitch), "revolutions": coil.turns,
                "clockwise": coil.clockwise, "name": "Coil It"}),
            operation,
        ),
    )
    .map_err(|e| super::super::refusal::refused(cmd, "the kernel could not wind the coil the scan shows", &e.to_string()))?;
    built(&view, Some(sketch), feature, solid, how, Some(coil.pitch * coil.turns), 0, 1)
}
