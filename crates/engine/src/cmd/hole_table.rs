//! Hole Table (`inspect.hole_table`): every round hole of the shown solids, read from their exact
//! faces, numbered in the view and listed (diameter, depth, through or blind, where it starts and
//! which way it runs), optionally written as CSV. A quick check of a part (a reconstructed scan
//! above all) against its drawing. A view: it changes nothing.
//!
//! A hole is the inward-facing cylindrical faces on one axis line, end to end: a hole split into
//! two half faces is one wall, and a counterbore and its bore are one hole with two steps. Whether
//! it goes through is decided exactly, by whether the body has material just past each of its
//! outer ends inside its radius (`material_in_ring`, as Thread decides a blind end).

use std::sync::Arc;

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use super::CommandSpec;
use crate::params::{bad, bool_, str_};
use crate::schema::hole_table::{HoleTableParams, HoleTableResult, HoleTableRow, HoleTableSkipped, HoleTableStep};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<HoleTableParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("inspect.hole_table", "Hole Table", crate::checked!("inspect.hole_table", hole_table))
    .at("INSPECT", "MEASURE")
    .icon("hole")
    .noundo()
    .contract(&CONTRACT)
    .output(output_schema_for::<HoleTableResult>)
    .params(
        "bodies?: [names] (default: every shown solid body); csv?: path (the table also written there, comma-separated, in mm); show?: bool (false takes the numbers away): every round hole read from the bodies' exact faces, numbered in the view, a view and not an undo step → shown, holes [{hole, body, kind (through|blind|closed), at, axis, steps [{diameter, depth}], depth}], skipped [{body, reason}], csv",
    )];

/// The holes `inspect.hole_table` last read, for the viewport to number. View state: not part of
/// the design and not an undo step; it stands for the design as it was at `revision`.
#[derive(Clone, Debug, PartialEq)]
pub struct HoleTable {
    pub revision: u64,
    /// Each hole's number, where it starts, and its label's facts.
    pub holes: Vec<ShownHole>,
}

/// A hole as the view numbers it.
#[derive(Clone, Debug, PartialEq)]
pub struct ShownHole {
    pub number: usize,
    pub at: Vec3,
    /// The diameters from the open end inwards, mm.
    pub diameters: Vec<f64>,
    pub kind: &'static str,
}

impl HoleTable {
    /// Whether it still stands for the design: nothing has changed since it was read.
    pub fn current(&self, s: &Session) -> bool {
        self.revision == s.revision
    }
}

/// One cylindrical wall: its axis line, radius and extent along the axis.
#[derive(Clone, Copy, Debug)]
struct Wall {
    cyl: solidcraft_kernel::CylinderFace,
}

impl Wall {
    fn point(&self, along: f64) -> Vec3 {
        self.cyl.axis_point + self.cyl.axis * along
    }
    fn axis(&self) -> Vec3 {
        *self.cyl.axis
    }
    /// Whether `other` lies on this wall's axis line, within `tol`; its extent then in this
    /// wall's axis coordinate.
    fn on_line(&self, other: &Wall, tol: f64) -> Option<(f64, f64)> {
        let d = self.axis().dot(other.axis());
        if d.abs() < 1.0 - 1e-9 {
            return None;
        }
        let off = other.cyl.axis_point - self.cyl.axis_point;
        if (off - self.axis() * off.dot(self.axis())).len() > tol {
            return None;
        }
        let s = off.dot(self.axis());
        let (a, b) = (s + other.cyl.start * d, s + other.cyl.end * d);
        Some((a.min(b), a.max(b)))
    }
}

/// A hole: walls on one axis line, end to end, in that line's coordinate (`base`'s axis).
struct Stack {
    base: Wall,
    /// Each wall's radius and extent along the base axis, in order along it.
    walls: Vec<(f64, f64, f64)>,
}

fn hole_table(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "inspect.hole_table";
    let reply = |r: HoleTableResult| serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()));
    if bool_(p, "show") == Some(false) {
        s.hole_table = None;
        return reply(HoleTableResult { shown: false, holes: Vec::new(), skipped: Vec::new(), csv: None, param_warnings: Vec::new() });
    }
    let asked: Option<Vec<String>> = match p.get("bodies").filter(|v| !v.is_null()) {
        None => None,
        Some(Value::Array(a)) => {
            Some(a.iter().map(|v| v.as_str().map(str::to_owned)).collect::<Option<Vec<_>>>().ok_or_else(|| bad(cmd, "`bodies` are body names"))?)
        }
        Some(_) => return Err(bad(cmd, "`bodies` is a list of body names")),
    };
    let csv = str_(p, "csv").map(str::trim).filter(|c| !c.is_empty()).map(str::to_owned);
    let state = s.model.state();
    if let Some(names) = &asked
        && let Some(missing) = names.iter().find(|n| state.body(n).is_none())
    {
        return Err(bad(cmd, format!("no body `{missing}`")));
    }
    let mut stacks: Vec<(String, Stack, &'static str, bool)> = Vec::new();
    let mut skipped = Vec::new();
    for b in &state.bodies {
        let wanted = match &asked {
            Some(names) => names.contains(&b.name),
            None => !b.body.is_mesh() && !s.visibility.hidden_bodies.contains(&b.name),
        };
        if !wanted {
            continue;
        }
        if b.body.is_mesh() {
            skipped.push(HoleTableSkipped { body: b.name.clone(), reason: "a mesh body has no exact faces to read holes from".into() });
            continue;
        }
        let tol = (b.body.size() * 1e-6).max(1e-6) * 100.0;
        let mut walls: Vec<Wall> = (0..b.body.face_count())
            .filter_map(|f| solidcraft_kernel::cylinder_face_exact(&b.body, f))
            .filter(|c| c.internal && c.radius > tol && c.end > c.start)
            .map(|cyl| Wall { cyl })
            .collect();
        // Larger first, so a stack's base is its widest wall (the counterbore over its bore).
        walls.sort_by(|a, b| b.cyl.radius.total_cmp(&a.cyl.radius));
        let mut body_stacks: Vec<Stack> = Vec::new();
        for w in walls {
            let joined = body_stacks.iter_mut().find_map(|st| {
                let (a, z) = st.base.on_line(&w, tol)?;
                // On the line: a piece of a wall already there (same radius, overlapping), or a
                // wall end to end with the stack.
                if let Some(same) = st.walls.iter_mut().find(|(r, s0, s1)| (r - w.cyl.radius).abs() <= tol && a <= *s1 + tol && z >= *s0 - tol) {
                    same.1 = same.1.min(a);
                    same.2 = same.2.max(z);
                    return Some(());
                }
                let lo = st.walls.iter().map(|w| w.1).fold(f64::INFINITY, f64::min);
                let hi = st.walls.iter().map(|w| w.2).fold(f64::NEG_INFINITY, f64::max);
                ((a - hi).abs() <= tol || (z - lo).abs() <= tol).then(|| st.walls.push((w.cyl.radius, a, z)))
            });
            if joined.is_none() {
                body_stacks.push(Stack { walls: vec![(w.cyl.radius, w.cyl.start, w.cyl.end)], base: w });
            }
        }
        for mut st in body_stacks {
            st.walls.sort_by(|a, b| a.1.total_cmp(&b.1));
            let (Some(first), Some(last)) = (st.walls.first().copied(), st.walls.last().copied()) else { continue };
            // Material just past each outer end, inside the wall's radius there.
            let past = |r: f64| (0.25 * r).clamp(1e-3, 0.5);
            let base = st.base.cyl;
            let ring = |r: f64, along: (f64, f64)| {
                let c = solidcraft_kernel::CylinderFace { radius: r, ..base };
                solidcraft_kernel::material_in_ring(&b.body, &c, 0.0, 0.9 * r, along)
            };
            let closed_lo = ring(first.0, (first.1 - past(first.0), first.1));
            let closed_hi = ring(last.0, (last.2, last.2 + past(last.0)));
            let (closed_lo, closed_hi) = match (closed_lo, closed_hi) {
                (Ok(a), Ok(b)) => (a, b),
                (Err(e), _) | (_, Err(e)) => {
                    skipped.push(HoleTableSkipped { body: b.name.clone(), reason: format!("a hole's ends could not be read: {e}") });
                    continue;
                }
            };
            let kind = match (closed_lo, closed_hi) {
                (false, false) => "through",
                (true, true) => "closed",
                _ => "blind",
            };
            // A blind hole starts at its open end and runs into the part; a hole through it at
            // its wider end (a counterbore's), as a drawing reads it.
            let wider_last = last.0 > first.0 + tol;
            let backward = (closed_lo && !closed_hi) || (kind == "through" && wider_last);
            if backward {
                st.walls.reverse();
            }
            stacks.push((b.name.clone(), st, kind, backward));
        }
    }
    drop(state);
    let mut rows: Vec<HoleTableRow> = stacks
        .into_iter()
        .map(|(body, st, kind, backward)| {
            let (start, axis) = match (backward, st.walls.first()) {
                (false, Some(f)) => (st.base.point(f.1), st.base.axis()),
                (true, Some(f)) => (st.base.point(f.2), -st.base.axis()),
                (_, None) => (st.base.cyl.axis_point, st.base.axis()),
            };
            let steps: Vec<HoleTableStep> = st.walls.iter().map(|(r, a, z)| HoleTableStep { diameter: 2.0 * r, depth: z - a }).collect();
            HoleTableRow {
                hole: 0,
                body,
                kind: kind.into(),
                at: [start.x, start.y, start.z],
                axis: [axis.x, axis.y, axis.z],
                depth: steps.iter().map(|s| s.depth).sum(),
                steps,
            }
        })
        .collect();
    // Numbered as read on a drawing: along X, then Y, then Z (to a hundredth of a millimetre).
    let key = |r: &HoleTableRow| r.at.map(|v| (v * 100.0).round() as i64);
    rows.sort_by(|a, b| key(a).cmp(&key(b)).then_with(|| a.body.cmp(&b.body)));
    for (i, r) in rows.iter_mut().enumerate() {
        r.hole = i + 1;
    }
    if let Some(path) = &csv {
        std::fs::write(path, csv_text(&rows)).map_err(|e| bad(cmd, format!("could not write `{path}`: {e}")))?;
    }
    s.hole_table = Some(Arc::new(HoleTable {
        revision: s.revision,
        holes: rows
            .iter()
            .map(|r| ShownHole {
                number: r.hole,
                at: Vec3::new(r.at[0], r.at[1], r.at[2]),
                diameters: r.steps.iter().map(|s| s.diameter).collect(),
                kind: match r.kind.as_str() {
                    "through" => "through",
                    "blind" => "blind",
                    _ => "closed",
                },
            })
            .collect(),
    }));
    reply(HoleTableResult { shown: true, holes: rows, skipped, csv, param_warnings: Vec::new() })
}

/// The table as comma-separated values, one line per diameter of each hole, in mm.
fn csv_text(rows: &[HoleTableRow]) -> String {
    let mut out = String::from("hole,body,kind,diameter,depth,step,step_depth,x,y,z,axis_x,axis_y,axis_z\n");
    for r in rows {
        let body = format!("\"{}\"", r.body.replace('"', "\"\""));
        for (i, st) in r.steps.iter().enumerate() {
            let [x, y, z] = r.at;
            let [ax, ay, az] = r.axis;
            out += &format!(
                "{},{body},{},{:.4},{:.4},{},{:.4},{x:.4},{y:.4},{z:.4},{ax:.6},{ay:.6},{az:.6}\n",
                r.hole,
                r.kind,
                st.diameter,
                r.depth,
                i + 1,
                st.depth
            );
        }
    }
    out
}

#[cfg(test)]
#[path = "hole_table_tests.rs"]
mod tests;
