//! Surface Check (`inspect.surface_check`): every B-spline face of the bodies chosen sampled on a
//! grid of its exact surface, cut to the face's trims, for what would make it a bad face of a part
//! (folds, pinches, the surface passing through itself), with how square and how even its
//! parameters are ([`solidcraft_scan::surface_check`]). It reads only: nothing changes and there is
//! no undo step. Faces of other kinds (planes, cylinders, …) cannot fold and are counted, not
//! checked.

use serde_json::Value;
use solidcraft_scan::surface_check::{self as checking, Check, GRID, MAX_GRID, MIN_GRID};

use super::CommandSpec;
use crate::params::{bad, string_list};
use crate::schema::surface_check::{SurfaceCheckFace, SurfaceCheckParams, SurfaceCheckPlace, SurfaceCheckResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};

const CONTRACT: TypedContract<SurfaceCheckParams> = TypedContract::new();

/// Most bodies, and most B-spline faces, one call checks.
const MAX_BODIES: usize = 64;
pub const MAX_FACES: usize = 2_000;

/// Most places of the surfaces one call evaluates (faces × (grid + 1)²): a few seconds at most.
const MAX_PLACES: usize = 4_000_000;

pub static COMMANDS: &[CommandSpec] =
    &[CommandSpec::new("inspect.surface_check", "Surface Check", crate::checked!("inspect.surface_check", surface_check))
        .contract(&CONTRACT)
        .output(output_schema_for::<SurfaceCheckResult>)
        .at("SURFACE", "INSPECT")
        .icon("inspect")
        .noundo()
        .params(
            "bodies?: [names] (default: the selected bodies, else every visible body that is not a mesh); grid?: cells each way on \
         each face, 4…200 (default 40): each B-spline face sampled on a grid of its exact surface, cut to its trims, for folds \
         (the surface doubling back), pinches (narrowing to no area away from a corner) and crossings (passing through \
         itself), with how square its parameters are (least angle, degrees) and how evenly they cover it (stretch, 1 is \
         even); reads only → faces [{body, face, clean, folds, pinches, crossings, cells, least_angle, stretch, places \
         [{kind, at}]}], checked, clean, skipped (faces of other kinds), bodies, summary",
        )];

/// The bodies to check: those named, else the selected ones, else every visible body that is not
/// a mesh.
fn chosen(s: &Session, p: &Value, st: &solidcraft_doc::ModelState, cmd: &str) -> Result<Vec<String>> {
    if p.get("bodies").is_some_and(|v| !v.is_null()) {
        let names = string_list(cmd, p, "bodies")?;
        if names.is_empty() || names.len() > MAX_BODIES {
            return Err(bad(cmd, format!("`bodies` must name 1…{MAX_BODIES} bodies")));
        }
        for n in &names {
            let b = st.body(n).ok_or_else(|| bad(cmd, format!("no body `{n}`")))?;
            if b.body.is_mesh() {
                return Err(bad(cmd, format!("`{n}` is a mesh body: it has no surfaces to check")));
            }
        }
        return Ok(names);
    }
    let mut selected: Vec<String> = Vec::new();
    for x in &s.selection {
        if let Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } = x
            && st.body(name).is_some_and(|b| !b.body.is_mesh())
            && !selected.contains(name)
        {
            selected.push(name.clone());
        }
    }
    if !selected.is_empty() {
        return Ok(selected);
    }
    let visible: Vec<String> =
        st.bodies.iter().filter(|b| !b.body.is_mesh() && s.body_visible(&b.name)).map(|b| b.name.clone()).take(MAX_BODIES).collect();
    if visible.is_empty() {
        return Err(bad(cmd, "the design has no body with surfaces to check"));
    }
    Ok(visible)
}

/// Whether a face is a B-spline surface (the kind that can fold).
fn is_spline(f: &solidcore_record::brep::Face) -> bool {
    matches!(f.surface, solidcore_record::brep::Surface::Spline { .. } | solidcore_record::brep::Surface::RationalSpline { .. })
}

/// Face `face` of `body` checked on a `grid` × `grid` grid cut to its trims, `None` when it is not
/// a B-spline surface.
pub fn check_face(body: &crate::kernel::Body, face: usize, grid: usize) -> Option<Check> {
    let f = body.record().faces.get(face).filter(|f| is_spline(f))?;
    let surface = super::scan::surface::nurbs_of_surface(f.parameters())?;
    let region = crate::kernel::face_region(body, face).ok()?;
    checking::check(&surface, grid, region.domain, &|u, v| region.contains(u, v))
}

fn surface_check(s: &mut Session, p: &Value) -> Result<Value> {
    serde_json::to_value(checked(s, p)?).map_err(|e| EngineError::Other(e.to_string()))
}

/// What `inspect.surface_check` finds for `p`, as a dialog shows it before anything is run.
pub fn checked(s: &Session, p: &Value) -> Result<SurfaceCheckResult> {
    let cmd = "inspect.surface_check";
    let grid = match p.get("grid").filter(|v| !v.is_null()) {
        None => GRID,
        Some(v) => v
            .as_f64()
            .filter(|g| g.fract() == 0.0 && (MIN_GRID as f64..=MAX_GRID as f64).contains(g))
            .map(|g| g as usize)
            .ok_or_else(|| bad(cmd, format!("`grid` must be a whole number of cells each way, {MIN_GRID}…{MAX_GRID}")))?,
    };
    let st = s.world_state()?;
    let bodies = chosen(s, p, &st, cmd)?;
    let splines: usize = bodies.iter().filter_map(|n| st.body(n)).map(|b| b.body.record().faces.iter().filter(|f| is_spline(f)).count()).sum();
    if splines > MAX_FACES {
        return Err(bad(cmd, format!("{splines} B-spline faces: at most {MAX_FACES} a call, so name fewer bodies in `bodies`")));
    }
    if splines.saturating_mul((grid + 1) * (grid + 1)) > MAX_PLACES {
        return Err(bad(cmd, format!("{splines} B-spline faces at a grid of {grid} is too much at once: choose a coarser grid or fewer bodies")));
    }
    let (mut faces, mut skipped) = (Vec::new(), 0);
    for name in &bodies {
        let Some(b) = st.body(name) else { continue };
        for (index, f) in b.body.record().faces.iter().enumerate() {
            if !is_spline(f) {
                skipped += 1;
                continue;
            }
            let Some(c) = check_face(&b.body, index, grid) else {
                skipped += 1;
                continue;
            };
            faces.push(SurfaceCheckFace {
                body: name.clone(),
                face: index,
                clean: c.is_clean(),
                folds: c.folds,
                pinches: c.pinches,
                crossings: c.crossings,
                cells: c.cells,
                least_angle: c.least_angle,
                stretch: if c.stretch.is_finite() { c.stretch } else { f64::MAX },
                places: c.places.iter().map(|(kind, at)| SurfaceCheckPlace { kind: kind.name().into(), at: [at.x, at.y, at.z] }).collect(),
            });
        }
    }
    let clean = faces.iter().filter(|f| f.clean).count();
    let summary = summary(&faces, skipped);
    Ok(SurfaceCheckResult { checked: faces.len(), clean, skipped, bodies, summary, faces, param_warnings: Vec::new() })
}

/// One line for a reader: how many faces were checked and what is wrong where.
fn summary(faces: &[SurfaceCheckFace], skipped: usize) -> String {
    let other = if skipped > 0 { format!(" ({skipped} of other kinds not checked)") } else { String::new() };
    if faces.is_empty() {
        return format!("no B-spline faces to check{other}");
    }
    let bad: Vec<String> = faces
        .iter()
        .filter(|f| !f.clean)
        .map(|f| {
            let mut what = Vec::new();
            for (n, one, many) in [(f.folds, "fold", "folds"), (f.pinches, "pinch", "pinches"), (f.crossings, "crossing", "crossings")] {
                if n > 0 {
                    what.push(format!("{n} {}", if n == 1 { one } else { many }));
                }
            }
            format!("{} face {}: {}", f.body, f.face, what.join(", "))
        })
        .collect();
    if bad.is_empty() {
        format!("{} B-spline face(s) checked{other}: all clean", faces.len())
    } else {
        format!("{} B-spline face(s) checked{other}: {} clean; {}", faces.len(), faces.len() - bad.len(), bad.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: Value) -> Value {
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    /// A surface body from a net (`n` × `n` points from `f(i, j)`), through Edit Surface's `net`
    /// form: New Free Form makes a surface, and the net replaces its points.
    fn surface_from(s: &mut Session, n: usize, f: impl Fn(usize, usize) -> [f64; 3]) -> String {
        let made = run(s, "scan.free_form", json!({"corners": [[0, 0, 0], [50, 0, 0], [50, 50, 0], [0, 50, 0]], "fit": "off", "count": n}));
        let body = made["surface"].as_str().unwrap_or_else(|| panic!("{made}")).to_owned();
        let mut knots = vec![0.0; 4];
        knots.extend((1..n - 3).map(|k| k as f64 / (n - 3) as f64));
        knots.extend([1.0; 4]);
        let points: Vec<[f64; 3]> = (0..n).flat_map(|j| (0..n).map(move |i| (i, j))).map(|(i, j)| f(i, j)).collect();
        run(s, "scan.edit_surface", json!({"surface": body, "net": {"count": [n, n], "knots": [knots.clone(), knots], "points": points}}));
        body
    }

    #[test]
    fn a_smooth_surface_is_clean_and_a_folded_one_says_where() {
        let mut s = Session::new(solidcraft_doc::Document::default());
        let smooth = surface_from(&mut s, 6, |i, j| [i as f64 * 10.0, j as f64 * 10.0, ((i * j) as f64).sqrt()]);
        let r = run(&mut s, "inspect.surface_check", json!({"bodies": [smooth]}));
        assert_eq!((r["checked"].as_u64(), r["clean"].as_u64()), (Some(1), Some(1)), "{r}");
        assert_eq!(r["faces"][0]["clean"], json!(true));
        assert_eq!(r["faces"][0]["cells"], json!(1600), "the whole face, 40 × 40");
        assert!(r["summary"].as_str().unwrap().contains("all clean"), "{r}");

        // A column dragged back past its neighbour folds the surface over.
        let folded = surface_from(&mut s, 6, |i, j| [if i == 3 { 0.0 } else { i as f64 * 10.0 }, j as f64 * 10.0, 0.0]);
        let r = run(&mut s, "inspect.surface_check", json!({"bodies": [folded.clone()], "grid": 20}));
        let face = &r["faces"][0];
        assert_eq!(face["clean"], json!(false), "{r}");
        assert!(face["folds"].as_u64().unwrap() > 0, "{r}");
        assert_eq!(face["cells"], json!(400));
        assert_eq!(face["places"][0]["kind"], json!("fold"));
        assert!(r["summary"].as_str().unwrap().contains(&format!("{folded} face 0")), "{r}");

        // With none named: the visible bodies, both of them.
        let r = run(&mut s, "inspect.surface_check", json!({}));
        assert_eq!((r["checked"].as_u64(), r["clean"].as_u64()), (Some(2), Some(1)), "{r}");
    }

    #[test]
    fn a_box_has_no_b_spline_faces_and_a_mesh_is_refused() {
        let mut s = Session::new(solidcraft_doc::Document::default());
        run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 10, "width": 10, "height": 10}));
        let r = run(&mut s, "inspect.surface_check", json!({"bodies": ["Block"]}));
        assert_eq!((r["checked"].as_u64(), r["skipped"].as_u64()), (Some(0), Some(6)), "{r}");
        assert!(r["summary"].as_str().unwrap().starts_with("no B-spline faces"), "{r}");
        let revision = s.revision;
        assert_eq!(s.revision, revision, "reads only");
    }

    #[test]
    fn hostile_parameters_are_refused() {
        let mut s = Session::new(solidcraft_doc::Document::default());
        run(&mut s, "solid.box", json!({"body_name": "Block", "operation": "new", "length": 10, "width": 10, "height": 10}));
        for p in [
            json!({"grid": 3}),
            json!({"grid": 201}),
            json!({"grid": 40.5}),
            json!({"grid": "40"}),
            json!({"grid": -1}),
            json!({"bodies": []}),
            json!({"bodies": ["Nope"]}),
            json!({"bodies": [1, 2]}),
        ] {
            assert!(s.execute("inspect.surface_check", &p).is_err(), "{p}");
        }
    }
}
