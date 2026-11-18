//! A first look at a scan (`scan.survey`): what it is and what is wrong with it, read without
//! changing the design, and the commands to run next in the order the work goes. It is where an
//! agent starts on a scan, and what a person reads to choose the next tool.

use serde_json::{Value, json};
use solidcraft_geom::Vec3Geom as _;
use solidcraft_scan::Scan;
use solidcraft_scan::fit::Kind;
use solidcraft_scan::survey::{Look, look};

use super::{CommandSpec, clean::FLOATING_SHARE, mesh_name};
use crate::params::bad;
use crate::schema::scan_survey::{ScanSurveyParams, ScanSurveyResult, ScanSurveyShapes, ScanSurveyStep};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanSurveyParams> = TypedContract::new();

/// A scan less than this across (mm) was probably saved in metres or inches.
const TINY: f64 = 3.0;
/// More triangles than this: the tools that read every triangle take a while.
const BIG: usize = 5_000_000;
/// Thinnest over longest below this, middle over longest above [`BROAD`] and at least [`ROOM`]
/// long: a wall or a panel (a van's side wall with its wheel well stands about a tenth of its
/// length deep).
const THIN: f64 = 0.15;
const BROAD: f64 = 0.2;
const ROOM: f64 = 1000.0;
/// Noise under this share of the tolerance is not worth smoothing.
const NOISE_WORTH: f64 = 0.1;
/// The share of the area in shapes above which Reconstruct is the route, and below which
/// surfaces fitted over the scan are.
const PRISMATIC: f64 = 0.6;
const ORGANIC: f64 = 0.3;

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.survey", "Survey Scan", crate::checked!("scan.survey", survey))
    .at("SCAN", "INSERT")
    .icon("inspect")
    .contract(&CONTRACT)
    .output(output_schema_for::<ScanSurveyResult>)
    .noundo()
    .params("body?: mesh body (default: the selected or only one): a first look at the scan, changing nothing: its size (own_size along its own directions, longest first), fineness, noise and the Denoise strength it would take, holes, rims, floating pieces, open edges no hole closes, how much of it is broad flat faces, whether it is sheet-like, what Find All read off it (once it has run), the design's other scans and models (those shown), and `next`: the commands to run next with why and parameters to start from, in the order the work goes → body, triangles, points, unusable, low, high, center, own_size, area, typical_edge, units_hint, noise, smoothing, tolerance, tolerance_set, holes, longest_hole, rims, open_edges, tangled_edges, crack_edges, crossing_faces, pieces, floating, floating_share, flat_share, sheet_like, shapes{in_shapes, planes, cylinders, spheres, cones, tori, freeform, regions}, other_scans, models, next[{command, why, params}]")];

/// A survey kept for the Survey dialog: the reply, for the scan as it was read.
#[derive(Debug)]
pub struct KeptSurvey {
    pub scan: String,
    held: [usize; 4],
    fingerprint: u64,
    /// Whether Find All's reading was in the survey.
    had_shapes: bool,
    pub reply: Value,
}

impl KeptSurvey {
    /// Whether it is of the scan as it is now: the same mesh, and Find All's reading in it when
    /// there is one now (a survey read before Find All ran is read again after).
    pub fn current(&self, s: &Session) -> bool {
        let same = super::with_shown(s, &self.scan, |p, t| {
            super::reconstruct::held(p, t) == self.held || super::reconstruct::fingerprint(p, t) == self.fingerprint
        })
        .unwrap_or(false);
        same && self.had_shapes == shapes_now(s, &self.scan)
    }
}

/// Whether Find All's reading of `scan` as it is now is in the session.
fn shapes_now(s: &Session, scan: &str) -> bool {
    s.scan_shapes.as_ref().is_some_and(|k| k.scan == scan && k.current(s))
}

fn survey(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.survey";
    let body = mesh_name(s, p, cmd)?;
    let (found, other_scans, models, held, fingerprint) = {
        let state = s.model.state();
        let b = state.body(&body).ok_or_else(|| bad(cmd, format!("no body `{body}`")))?;
        if !b.body.is_mesh() {
            return Err(bad(cmd, format!("`{body}` is not a scan (a mesh body)")));
        }
        // Where the scan shows: its box and the places it names are the world's.
        let (found, held, fingerprint) = super::with_shown(s, &body, |positions, triangles| {
            let found = look(&Scan::new(positions, triangles), FLOATING_SHARE);
            (found, super::reconstruct::held(positions, triangles), super::reconstruct::fingerprint(positions, triangles))
        })
        .ok_or_else(|| bad(cmd, format!("`{body}` is not a scan (a mesh body)")))?;
        // Only what shows: a hidden scan (another part's, put away) is no pass to lay on this one,
        // and a hidden model none to check it against.
        let (scans, models): (Vec<_>, Vec<_>) =
            state.bodies.iter().filter(|b| b.name != body && s.body_visible(&b.name)).partition(|b| b.body.is_mesh());
        (
            found,
            scans.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            models.iter().map(|b| b.name.clone()).collect::<Vec<_>>(),
            held,
            fingerprint,
        )
    };
    let found = found.ok_or_else(|| bad(cmd, format!("`{body}` has no triangle with area to read")))?;
    let shapes = s.scan_shapes.clone().filter(|k| k.scan == body && k.current(s)).map(|k| shares(&k.outcome));
    let tolerance = super::tolerance::of(s, &body);
    let tolerance_set = super::tolerance::is_set(s, &body);
    let across = (found.high - found.low).len();
    let units_hint = (across < TINY)
        .then(|| format!("{across:.3} mm across: the file was probably saved in metres or inches; mesh.units scales it to its own unit"));
    let [long, middle, thin] = found.own_size;
    let sheet_like = long >= ROOM && thin / long < THIN && middle / long > BROAD;
    let next = next_steps(&Plan {
        sizes: found.own_size,
        body: &body,
        look: &found,
        tiny: units_hint.is_some(),
        sheet_like,
        shapes: shapes.as_ref(),
        tolerance,
        tolerance_set,
        other_scans: &other_scans,
        models: &models,
    });
    let xyz = |v: solidcore_record::Vec3| [v.x, v.y, v.z];
    let reply = ScanSurveyResult {
        body,
        triangles: found.triangle_count,
        points: found.points,
        unusable: found.unusable,
        low: xyz(found.low),
        high: xyz(found.high),
        center: xyz(found.centre),
        own_size: found.own_size,
        area: found.area,
        typical_edge: found.typical_edge,
        units_hint,
        noise: found.noise,
        smoothing: found.smoothing,
        tolerance,
        tolerance_set,
        holes: found.holes,
        longest_hole: found.longest_hole,
        rims: found.rims,
        open_edges: found.open_edges,
        tangled_edges: found.tangled_edges,
        crack_edges: found.crack_edges,
        crossing_faces: found.crossing_faces,
        pieces: found.pieces,
        floating: found.floating,
        floating_share: found.floating_share.max(0.0),
        flat_share: found.flat_share.max(0.0),
        sheet_like,
        shapes,
        other_scans,
        models,
        next,
        param_warnings: Vec::new(),
    };
    let had_shapes = reply.shapes.is_some();
    let reply = serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))?;
    s.scan_survey = Some(std::sync::Arc::new(KeptSurvey {
        scan: reply["body"].as_str().unwrap_or_default().to_owned(),
        held,
        fingerprint,
        had_shapes,
        reply: reply.clone(),
    }));
    Ok(reply)
}

/// Find All's regions as shares of the area by kind.
fn shares(outcome: &solidcraft_scan::segment::Outcome) -> ScanSurveyShapes {
    let total: f64 = outcome.segments.iter().map(|g| g.area).sum::<f64>().max(f64::MIN_POSITIVE);
    let of = |kind: Option<Kind>| outcome.segments.iter().filter(|g| g.shape.as_ref().map(|s| s.kind()) == kind).map(|g| g.area).sum::<f64>() / total;
    ScanSurveyShapes {
        in_shapes: outcome.in_shapes,
        planes: of(Some(Kind::Plane)),
        cylinders: of(Some(Kind::Cylinder)),
        spheres: of(Some(Kind::Sphere)),
        cones: of(Some(Kind::Cone)),
        tori: of(Some(Kind::Torus)),
        freeform: of(None),
        regions: outcome.segments.len(),
    }
}

/// What the next steps are worked out from.
struct Plan<'a> {
    body: &'a str,
    sizes: [f64; 3],
    look: &'a Look,
    tiny: bool,
    sheet_like: bool,
    shapes: Option<&'a ScanSurveyShapes>,
    tolerance: f64,
    tolerance_set: bool,
    other_scans: &'a [String],
    models: &'a [String],
}

/// The commands worth running next, in the order the work goes: units, clean, align, read,
/// build, check.
fn next_steps(plan: &Plan) -> Vec<ScanSurveyStep> {
    let mut next = Vec::new();
    let mut step = |command: &str, why: String, params: Value| next.push(ScanSurveyStep { command: command.into(), why, params: Some(params) });
    let (look, body) = (plan.look, plan.body);
    let [long, middle, thin] = plan.sizes;
    if plan.tiny {
        step(
            "mesh.units",
            "the scan is only a few millimetres across: scale it from the unit its file was saved in (m or in)".into(),
            json!({"body": body, "units": "m"}),
        );
    }
    if look.tangled_edges > 0 || look.unusable > 0 || look.crossing_faces > 0 {
        let edges = match (look.crack_edges, look.tangled_edges - look.crack_edges.min(look.tangled_edges)) {
            (0, tangled) => {
                format!("{tangled} open edges belong to no hole (edges of three triangles) and {} triangles are unusable", look.unusable)
            }
            (cracks, 0) => {
                format!("{cracks} open edges are cracks between points a hair apart (a file's two copies of one point): Repair Mesh joins them")
            }
            (cracks, tangled) => format!(
                "{cracks} open edges are cracks between points a hair apart, {tangled} belong to no hole and {} triangles are unusable",
                look.unusable
            ),
        };
        let why = match (look.tangled_edges > 0 || look.unusable > 0, look.crossing_faces) {
            (true, 0) => edges,
            (false, crossing) => {
                format!("{crossing} faces pass through other faces (folds or overlapping passes): Repair Mesh takes them out and fills round them")
            }
            (true, crossing) => format!("{edges}; {crossing} faces pass through other faces"),
        };
        step("scan.repair", why, json!({"body": body}));
    }
    if look.floating > 0 {
        step(
            "scan.remove_floating",
            format!(
                "{} pieces float apart from the part ({:.2}% of the area): dust, fixtures or reflections",
                look.floating,
                100.0 * look.floating_share
            ),
            json!({"body": body}),
        );
    }
    if !plan.other_scans.is_empty() {
        let mut bodies = vec![body.to_owned()];
        bodies.extend(plan.other_scans.iter().cloned());
        step(
            "scan.align_all",
            format!("the design has {} other scan(s): align them onto this one (scan.combine also fuses them into one mesh)", plan.other_scans.len()),
            json!({"bodies": bodies}),
        );
    }
    // Smoothing and patched holes help surfaces fitted over the scan; Reconstruct fits shapes
    // through the noise and the holes, and on the T-LESS benchmark it read worse after either. So
    // they are suggested once Find All has shown the scan is for fitted surfaces.
    let surfaces = !plan.sheet_like && plan.shapes.is_some_and(|sh| sh.in_shapes < PRISMATIC);
    let worth_smoothing = look.noise.is_some_and(|n| n > NOISE_WORTH * plan.tolerance);
    if let Some(strength) = plan.look.smoothing.filter(|s| *s > 0.0 && worth_smoothing && surfaces) {
        step(
            "scan.denoise",
            format!("its noise ({:.3} mm) smooths away with strength {strength:.2} without rounding sharp edges", look.noise.unwrap_or(0.0)),
            json!({"body": body}),
        );
    }
    // On a wall or a panel the holes are mostly real: the frame's holes Flat Template reads.
    if look.holes > 0 && surfaces {
        step(
            "scan.fill_holes",
            format!(
                "{} holes inside its rims, the longest {:.1} mm round (the default closes those of up to 300 edges)",
                look.holes, look.longest_hole
            ),
            json!({"body": body}),
        );
    }
    if let Some(noise) = look.noise.filter(|n| !plan.tolerance_set && 3.0 * n > plan.tolerance) {
        let suggested = super::tolerance::for_noise(noise);
        step(
            "scan.tolerance",
            format!(
                "its noise ({noise:.3} mm) is more than a third of the {:.2} mm tolerance every check reads against: most of a faithful fit would read as off",
                plan.tolerance
            ),
            json!({"scan": body, "tolerance": suggested}),
        );
    }
    let contains_origin = [look.low.x <= 0.0 && look.high.x >= 0.0, look.low.y <= 0.0 && look.high.y >= 0.0, look.low.z <= 0.0 && look.high.z >= 0.0];
    if contains_origin.iter().any(|c| !c) && !plan.sheet_like {
        let (command, why) = if look.flat_share > 0.2 {
            ("scan.stand_on_base", "the scan lies away from the origin: stand it on its broadest flat face so sketches and planes line up with it")
        } else {
            ("scan.quick_align", "the scan lies away from the origin: lay it along its own directions with its middle at the origin")
        };
        step(command, why.into(), json!({"body": body}));
    }
    if look.triangle_count > BIG && !plan.sheet_like {
        step(
            "scan.select",
            format!(
                "{} triangles: read a part at a time (select it, then scan.reconstruct_selection) or thin it with scan.reduce",
                look.triangle_count
            ),
            json!({"body": body}),
        );
    }
    if plan.sheet_like {
        step(
            "scan.flat_template",
            format!(
                "the scan is a wall or a panel ({:.0} × {:.0} mm, {:.0} mm deep): a flat template with the cut-outs for what stands off it; its {} holes are left open, as on a wall they are mostly the frame's own (`panel` adds a straight sheet-cut panel)",
                long, middle, thin, look.holes
            ),
            json!({"body": body, "outline": "scan"}),
        );
    }
    match plan.shapes {
        None if !plan.sheet_like => step(
            "scan.find_all",
            "read which regions are planes, cylinders, spheres, cones, tori and freeform; run scan.survey again after it for the route".into(),
            json!({"body": body}),
        ),
        Some(sh) if sh.in_shapes >= PRISMATIC => step(
            "scan.reconstruct",
            format!(
                "{:.0}% of the area is planes, cylinders and other shapes: read the features (extrusions, revolves, cuts, fillets) off them",
                100.0 * sh.in_shapes
            ),
            json!({"body": body}),
        ),
        Some(sh) if sh.in_shapes < ORGANIC => step(
            "scan.fit_patches",
            format!(
                "only {:.0}% of the area is simple shapes: fit smooth surfaces over it (scan.free_form for hand-laid faces)",
                100.0 * sh.in_shapes
            ),
            json!({"body": body}),
        ),
        Some(sh) => step(
            "scan.reconstruct",
            format!(
                "{:.0}% of the area is simple shapes: read the features off them, then fit surfaces (scan.fit_patches) over the freeform rest",
                100.0 * sh.in_shapes
            ),
            json!({"body": body}),
        ),
        None => {}
    }
    if let Some(model) = plan.models.first() {
        step(
            "scan.deviation",
            format!("measure how far `{model}` is from the scan; `passes` says whether it is within the tolerance"),
            json!({"scan": body, "body": model}),
        );
    }
    next
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: Value) -> Value {
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
    }

    /// A closed box `size` from `at`, each side a grid of 16 × 16 squares, written as an ASCII STL.
    fn box_stl(dir: &std::path::Path, name: &str, size: [f64; 3], at: [f64; 3]) -> std::path::PathBuf {
        const N: usize = 16;
        let mut text = String::from("solid b\n");
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for high in [false, true] {
                let corner = |i: usize, j: usize| {
                    let mut q = at;
                    if high {
                        q[axis] += size[axis];
                    }
                    q[u] += size[u] * i as f64 / N as f64;
                    q[v] += size[v] * j as f64 / N as f64;
                    q
                };
                for j in 0..N {
                    for i in 0..N {
                        let (a, b, c, d) = (corner(i, j), corner(i + 1, j), corner(i + 1, j + 1), corner(i, j + 1));
                        let tris = if high { [[a, b, c], [a, c, d]] } else { [[a, c, b], [a, d, c]] };
                        for tri in tris {
                            text.push_str("facet normal 0 0 0\nouter loop\n");
                            for q in tri {
                                text.push_str(&format!("vertex {} {} {}\n", q[0], q[1], q[2]));
                            }
                            text.push_str("endloop\nendfacet\n");
                        }
                    }
                }
            }
        }
        text.push_str("endsolid b\n");
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    fn commands(r: &Value) -> Vec<&str> {
        r["next"].as_array().unwrap().iter().filter_map(|n| n["command"].as_str()).collect()
    }

    /// A clean box off the origin: read as built, nothing to clean, placing then Find All next;
    /// after Find All the route is Reconstruct; a second scan and a model add aligning and
    /// checking; the survey never changes the design.
    #[test]
    fn a_survey_reads_the_scan_and_says_what_to_run_next() {
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-survey-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(solidcraft_doc::Document::new("t"));
        run(&mut s, "file.insert_mesh", json!({"path": box_stl(&dir, "a.stl", [40.0, 20.0, 10.0], [100.0, 0.0, 0.0])}));
        let r = run(&mut s, "scan.survey", json!({}));
        assert_eq!((r["holes"].as_u64(), r["floating"].as_u64(), r["pieces"].as_u64()), (Some(0), Some(0), Some(1)), "{r}");
        assert!((r["area"].as_f64().unwrap() - 2800.0).abs() < 1e-6, "{r}");
        assert!((r["own_size"][0].as_f64().unwrap() - 40.0).abs() < 1e-6, "{r}");
        assert!(r.get("units_hint").is_none() && r["sheet_like"] == false && r.get("shapes").is_none(), "{r}");
        let next = commands(&r);
        assert_eq!(next, ["scan.stand_on_base", "scan.find_all"], "{r}");
        for step in r["next"].as_array().unwrap() {
            s.execute(step["command"].as_str().unwrap(), &step["params"]).unwrap_or_else(|e| panic!("{step}: {e}"));
        }
        let r = run(&mut s, "scan.survey", json!({}));
        let shapes = &r["shapes"];
        assert!(shapes["in_shapes"].as_f64().unwrap() > 0.9 && shapes["planes"].as_f64().unwrap() > 0.9, "{r}");
        assert_eq!(commands(&r), ["scan.reconstruct"], "{r}");

        run(&mut s, "file.insert_mesh", json!({"path": box_stl(&dir, "b.stl", [40.0, 20.0, 10.0], [0.0, 0.0, 0.0])}));
        run(&mut s, "solid.box", json!({"length": 10, "width": 10, "height": 10}));
        let scan = r["body"].as_str().unwrap().to_owned();
        let features = s.doc.features.len();
        let r = run(&mut s, "scan.survey", json!({"body": scan}));
        let next = commands(&r);
        assert!(next.contains(&"scan.align_all") && next.last() == Some(&"scan.deviation"), "{r}");
        assert_eq!((r["other_scans"].as_array().unwrap().len(), r["models"].as_array().unwrap().len()), (1, 1), "{r}");
        assert_eq!(s.doc.features.len(), features, "a survey changes nothing");
        assert!(s.execute("scan.survey", &json!({"body": "nope"})).is_err());
        assert!(s.execute("scan.survey", &json!({"body": 3})).is_err());
        // The other scan and the model hidden (put away): no longer passes to align, nor a model.
        let others: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).filter(|n| *n != scan).collect();
        run(&mut s, "browser.visibility", json!({"items": others, "visible": false}));
        let r = run(&mut s, "scan.survey", json!({"body": scan}));
        assert!(!commands(&r).contains(&"scan.align_all") && !commands(&r).contains(&"scan.deviation"), "{r}");
        assert_eq!((r["other_scans"].as_array().unwrap().len(), r["models"].as_array().unwrap().len()), (0, 0), "{r}");
    }

    /// A scan a few millimetres across is flagged as read in the wrong unit, and mesh.units is
    /// suggested first.
    #[test]
    fn a_tiny_scan_is_told_to_check_its_units() {
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-survey-tiny-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(solidcraft_doc::Document::new("t"));
        run(&mut s, "file.insert_mesh", json!({"path": box_stl(&dir, "m.stl", [0.4, 0.2, 0.1], [-0.2, -0.1, 0.0])}));
        let r = run(&mut s, "scan.survey", json!({}));
        assert!(r["units_hint"].is_string(), "{r}");
        assert_eq!(commands(&r).first(), Some(&"mesh.units"), "{r}");
    }

    /// A broad thin scan a few metres long reads as a wall: Flat Template is the route, and it is
    /// not sent to Find All, Stand on Base or a selection however it lies.
    #[test]
    fn a_wall_goes_to_flat_template() {
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-survey-wall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Session::new(solidcraft_doc::Document::new("t"));
        run(&mut s, "file.insert_mesh", json!({"path": box_stl(&dir, "w.stl", [3000.0, 1500.0, 200.0], [500.0, 500.0, 500.0])}));
        let r = run(&mut s, "scan.survey", json!({}));
        assert_eq!(r["sheet_like"], true, "{r}");
        assert_eq!(commands(&r), ["scan.flat_template"], "{r}");
    }

    /// Denoise and Fill Holes wait for Find All: before it a holey, noisy scan is sent to Find All
    /// only; read as mostly freeform it gets both before Fit Patches; read as shapes, neither.
    #[test]
    fn smoothing_and_holes_wait_for_the_route() {
        use solidcore_record::Vec3;
        let look = solidcraft_scan::survey::Look {
            triangle_count: 1000,
            points: 600,
            unusable: 0,
            low: Vec3::new(-10.0, -10.0, -10.0),
            high: Vec3::new(10.0, 10.0, 10.0),
            centre: Vec3::ZERO,
            own_size: [20.0, 20.0, 20.0],
            area: 2400.0,
            typical_edge: 1.0,
            noise: Some(0.05),
            smoothing: Some(1.0),
            noise_places: 8,
            rims: 0,
            holes: 3,
            longest_hole: 12.0,
            open_edges: 30,
            tangled_edges: 0,
            crack_edges: 0,
            crossing_faces: 0,
            pieces: 1,
            floating: 0,
            floating_share: 0.0,
            flat_share: 0.0,
        };
        let shares = |in_shapes: f64| super::ScanSurveyShapes {
            in_shapes,
            planes: in_shapes,
            cylinders: 0.0,
            spheres: 0.0,
            cones: 0.0,
            tori: 0.0,
            freeform: 1.0 - in_shapes,
            regions: 4,
        };
        let run = |shapes: Option<&super::ScanSurveyShapes>| -> Vec<String> {
            let plan = super::Plan {
                body: "s",
                sizes: look.own_size,
                look: &look,
                tiny: false,
                sheet_like: false,
                shapes,
                tolerance: 0.2,
                tolerance_set: true,
                other_scans: &[],
                models: &[],
            };
            super::next_steps(&plan).into_iter().map(|n| n.command).collect()
        };
        assert_eq!(run(None), ["scan.find_all"]);
        assert_eq!(run(Some(&shares(0.1))), ["scan.denoise", "scan.fill_holes", "scan.fit_patches"]);
        assert_eq!(run(Some(&shares(0.9))), ["scan.reconstruct"]);

        // Faces passing through each other are worth a Repair even with every edge in a ring: a
        // scan-like block with 253 such pairs lost a step of Reconstruct to the kernel's join.
        let crossing = solidcraft_scan::survey::Look { crossing_faces: 507, ..look.clone() };
        let plan = super::Plan {
            body: "s",
            sizes: crossing.own_size,
            look: &crossing,
            tiny: false,
            sheet_like: false,
            shapes: None,
            tolerance: 0.2,
            tolerance_set: true,
            other_scans: &[],
            models: &[],
        };
        let next = super::next_steps(&plan);
        assert_eq!(next.first().map(|n| n.command.as_str()), Some("scan.repair"));
        assert!(next[0].why.contains("507 faces pass through"), "{}", next[0].why);
    }
}
