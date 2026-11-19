//! Reconstruct (`scan.reconstruct`): what Find All's shapes make, and the steps that build it as
//! ordinary features. The button reads the scan first (Find All runs when it has not, and the scan
//! is coloured by its regions); the dialog then lists each step Reconstruct proposes (Revolve It
//! for a turned part, Extrude It for each prism, Cut It for each round hole and Pattern It for a
//! row or ring of them, Fillet It for the rounds no profile holds), all ticked, and OK builds the
//! ticked ones in order, each its own undo step. Reconstruct Selection
//! (`scan.reconstruct_selection`) is the same on the part of the scan selected, read as a part of
//! its own: a part too busy to read whole is built a feature at a time.

use serde_json::Value;
use solidcraft_engine::cmd::scan::reconstruct::{self, Proposal, Reading, ScanShapes};

use crate::SolveApp;
use crate::dialogs::Kind;
use crate::ui_kit;

/// The Reconstruct dialog's values: which steps are ticked, by their place in the proposals, and
/// whether it reads only the part of the scan selected (Reconstruct Selection).
#[derive(Clone, Debug, Default)]
pub struct Reconstruct {
    pub ticked: Vec<bool>,
    pub selected: bool,
    /// After the steps, Reconstruct the Rest: what the body leaves of the scan unexplained, read
    /// and built piece by piece. Off unless ticked.
    pub rest: bool,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    matches!(id, "scan.reconstruct" | "scan.reconstruct_selection")
        .then(|| (Kind::ScanReconstruct(Reconstruct { selected: id == "scan.reconstruct_selection", ..Reconstruct::default() }), Vec::new()))
}

/// Read the scan (or with `selected`, the part of it selected) as the dialog opens: false (and
/// nothing opens) when it cannot be read. Find All can take most of a minute on a big scan, so it
/// reads beside the window when that takes long, and the dialog lists what to build once the
/// reading is in.
pub fn read_first(app: &mut SolveApp, selected: bool) -> bool {
    if selected && solidcraft_engine::cmd::scan::select::selected(&app.session).is_none_or(|k| k.count == 0) {
        app.set_status(tr("ui.scan_reconstruct_selection_none").to_string(), true);
        return false;
    }
    let params = if selected { serde_json::json!({"selected": true}) } else { serde_json::json!({}) };
    match app.run_heavy("scan.reconstruct", params) {
        Ok(_) => true,
        Err(why) => {
            app.set_status(why, true);
            false
        }
    }
}

/// Whether the scan is being read beside the window for the dialog.
fn reading(app: &SolveApp) -> bool {
    app.computing_command() == Some("scan.reconstruct")
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

fn found(app: &SolveApp) -> Option<std::sync::Arc<ScanShapes>> {
    app.session.scan_shapes.clone().filter(|k| k.current(&app.session))
}

/// The reading the dialog lists the steps of, and the share of what was read in shapes: the whole
/// scan's, or (`selected`) the part of it selected now, once that has been read.
fn reading_of(app: &SolveApp, found: &ScanShapes, selected: bool) -> Option<(Reading, f64)> {
    if !selected {
        return Some((found.reading.clone(), found.outcome.in_shapes));
    }
    found.selected_region(&app.session).map(|r| (r.region.reading.clone(), r.region.outcome.in_shapes))
}

/// A step's line in the list.
fn label(app: &SolveApp, reading: &Reading, step: &Proposal) -> String {
    let mm = |v: f64| crate::prefs::show_mm(app, v, 1);
    match *step {
        Proposal::Revolve => {
            let faces = reading.revolve.as_ref().map_or(0, |r| r.members.len());
            solidcraft_ui_text::format("ui.scan_reconstruct_revolve", &[("faces", &faces.to_string())])
        }
        Proposal::Extrude(k) => {
            let (depth, walls) = reading.prisms.get(k).map_or((0.0, 0), |p| (p.depth(), p.walls.len()));
            solidcraft_ui_text::format(
                "ui.scan_reconstruct_extrude",
                &[("number", &(k + 1).to_string()), ("depth", &mm(depth)), ("walls", &walls.to_string())],
            )
        }
        Proposal::Cut(k) => {
            let (diameter, depth) = reading.holes.get(k).map_or((0.0, 0.0), |h| (2.0 * h.radius, h.depth()));
            solidcraft_ui_text::format(
                "ui.scan_reconstruct_cut",
                &[("number", &(k + 1).to_string()), ("diameter", &mm(diameter)), ("depth", &mm(depth))],
            )
        }
        Proposal::Hole(k) => {
            let hole = reading.holes.get(k);
            let form = hole.and_then(|h| h.form.as_ref());
            let kind = match form {
                Some(f) if f.counterbore.is_some() => "ui.scan_reconstruct_hole_counterbore",
                Some(f) if f.countersink.is_some() => "ui.scan_reconstruct_hole_countersink",
                Some(f) if f.thread.is_some() => "ui.scan_reconstruct_hole_tapped",
                _ => "ui.scan_reconstruct_hole_drilled",
            };
            let diameter = hole.map_or(0.0, |h| 2.0 * h.radius);
            solidcraft_ui_text::format(
                "ui.scan_reconstruct_hole",
                &[("number", &(k + 1).to_string()), ("kind", tr(kind)), ("diameter", &mm(diameter))],
            )
        }
        Proposal::PatternPrism(k) => {
            let count = reading.prism_repeats.iter().find(|r| r.members.first() == Some(&k)).map_or(0, |r| r.members.len());
            solidcraft_ui_text::format("ui.scan_reconstruct_pattern_prism", &[("number", &(k + 1).to_string()), ("count", &count.to_string())])
        }
        Proposal::Pattern(k) => {
            let count = reading.holes.get(k).and_then(|h| h.repeat).map_or(0, |r| r.0);
            solidcraft_ui_text::format("ui.scan_reconstruct_pattern", &[("number", &(k + 1).to_string()), ("count", &count.to_string())])
        }
        Proposal::Coil => {
            let (pitch, turns) = reading.coil.as_ref().map_or((0.0, 0.0), |c| (c.pitch, c.turns));
            solidcraft_ui_text::format("ui.scan_reconstruct_coil", &[("pitch", &mm(pitch)), ("turns", &format!("{turns}"))])
        }
        Proposal::Pipe => {
            let diameter = reading.pipe.as_ref().map_or(0.0, |p| 2.0 * p.radius);
            let bends = reconstruct::pipe_bends(reading);
            solidcraft_ui_text::format("ui.scan_reconstruct_pipe", &[("diameter", &mm(diameter)), ("bends", &bends.to_string())])
        }
        Proposal::Loft => {
            let sections = reading.loft.as_ref().map_or(0, |l| l.sections.len());
            solidcraft_ui_text::format("ui.scan_reconstruct_loft", &[("sections", &sections.to_string())])
        }
        Proposal::Fillet => solidcraft_ui_text::format("ui.scan_reconstruct_fillet", &[("edges", &reading.fillets.len().to_string())]),
        Proposal::Chamfer => solidcraft_ui_text::format("ui.scan_reconstruct_chamfer", &[("edges", &reading.chamfers.len().to_string())]),
        Proposal::Shell => {
            solidcraft_ui_text::format("ui.scan_reconstruct_shell", &[("thickness", &mm(reading.shell.as_ref().map_or(0.0, |w| w.thickness)))])
        }
    }
}

/// The handle of step `n`'s row: `scan_reconstruct:<n>:<command>`.
pub fn step_handle(n: usize, step: &Proposal) -> String {
    format!("scan_reconstruct:{n}:{}", step.command())
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Reconstruct) -> bool {
    let Some((found, (reading, in_shapes))) = found(app).and_then(|f| reading_of(app, &f, k.selected).map(|r| (f, r))) else {
        if reading(app) {
            ui_kit::text(ui, tr("ui.scan_reconstruct_reading"), ui_kit::Text::Note("scan_reconstruct_reading"));
        } else {
            ui_kit::text(ui, tr("ui.scan_reconstruct_unread"), ui_kit::Text::Report("scan_reconstruct_unread", true));
        }
        return false;
    };
    let steps = reconstruct::proposals(&reading, reconstruct::is_built(&app.session, &found.scan));
    k.ticked.resize(steps.len(), true);
    let found_id = if k.selected { "ui.scan_reconstruct_found_selected" } else { "ui.scan_reconstruct_found" };
    let summary =
        solidcraft_ui_text::format(found_id, &[("shapes", &reading.faces.len().to_string()), ("share", &format!("{:.0} %", 100.0 * in_shapes))]);
    ui_kit::text(ui, &summary, ui_kit::Text::Note("scan_reconstruct_found"));
    if steps.is_empty() {
        ui_kit::text(ui, tr("ui.scan_reconstruct_nothing"), ui_kit::Text::Report("scan_reconstruct_nothing", true));
    }
    // Each row has a handle of its own (its place and its command), so a script or a test ticks
    // the step it means: the rows' labels carry the scan's numbers and say nothing stable.
    for (n, (step, ticked)) in steps.iter().zip(k.ticked.iter_mut()).enumerate() {
        let row = ui_kit::checkbox(ui, &label(app, &reading, step), ticked);
        crate::scenario::publish_handle(&step_handle(n, step), row.rect.center());
    }
    if !k.selected && (!steps.is_empty() || reconstruct::is_built(&app.session, &found.scan)) {
        let row = ui_kit::checkbox(ui, tr("ui.scan_reconstruct_rest"), &mut k.rest);
        crate::scenario::publish_handle("scan_reconstruct:rest", row.rect.center());
    }
    ui_kit::text(ui, tr("ui.scan_reconstruct_hint"), ui_kit::Text::Hint("scan_reconstruct_hint"));
    false
}

/// What OK runs: the ticked steps, in order.
pub fn commands(app: &SolveApp, k: &Reconstruct) -> Result<Vec<(String, Value)>, String> {
    let found = found(app).ok_or_else(|| tr("ui.scan_reconstruct_unread").to_string())?;
    let (reading, _) = reading_of(app, &found, k.selected).ok_or_else(|| tr("ui.scan_reconstruct_unread").to_string())?;
    let chosen: Vec<(String, Value)> = reconstruct::proposals(&reading, reconstruct::is_built(&app.session, &found.scan))
        .iter()
        .zip(k.ticked.iter().copied().chain(std::iter::repeat(true)))
        .filter(|(_, ticked)| *ticked)
        .map(|(step, _)| (step.command().to_string(), step.params(&found.scan, k.selected)))
        .collect();
    let mut chosen = chosen;
    if k.rest && !k.selected {
        chosen.push(("scan.reconstruct_rest".to_string(), serde_json::json!({"body": found.scan})));
    }
    if chosen.is_empty() {
        return Err(tr("ui.scan_reconstruct_none_ticked").into());
    }
    Ok(chosen)
}

/// For each of the commands OK runs, the earlier one it builds on: Pattern It repeats the cut,
/// hole or extrusion of the same hole or prism, and is left out with it.
pub fn needs(commands: &[(String, Value)]) -> Vec<Option<usize>> {
    let same = |a: &Value, b: &Value, key: &str| a.get(key).is_some_and(|k| !k.is_null() && b.get(key) == Some(k));
    commands
        .iter()
        .enumerate()
        .map(|(i, (id, p))| {
            if id != "scan.pattern_it" {
                return None;
            }
            commands[..i].iter().rposition(|(made, q)| match made.as_str() {
                "scan.cut_it" | "scan.hole_it" => same(p, q, "hole"),
                "scan.extrude_it" => same(p, q, "prism"),
                _ => false,
            })
        })
        .collect()
}

/// What the status line says when OK left steps out: how many were built, and each step left out
/// with why.
pub fn left_out_note(built: usize, left_out: &[(String, String)]) -> String {
    let steps: Vec<String> = left_out.iter().map(|(id, why)| crate::friendly_error(id, why)).collect();
    solidcraft_ui_text::format(
        "ui.scan_reconstruct_left_out",
        &[("built", &built.to_string()), ("asked", &(built + left_out.len()).to_string()), ("steps", &steps.join("; "))],
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A slab 30 × 20 × 6, each face a fine grid, inserted as a scan.
    fn slab(h: &mut crate::scenario::Harness) {
        let (n, size) = (10u32, [30.0, 20.0, 6.0]);
        let mut stl = String::from("solid s\n");
        for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for j in 0..n {
                for i in 0..n {
                    let at = |di: u32, dj: u32| {
                        let mut p = [0.0; 3];
                        p[axis] = if high { size[axis] } else { 0.0 };
                        p[u] = size[u] * f64::from(i + di) / f64::from(n);
                        p[v] = size[v] * f64::from(j + dj) / f64::from(n);
                        p
                    };
                    let quad = [at(0, 0), at(1, 0), at(1, 1), at(0, 1)];
                    for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                        let tri = if high { tri } else { [tri[0], tri[2], tri[1]] };
                        stl += "facet normal 0 0 0\nouter loop\n";
                        for [x, y, z] in tri {
                            stl += &format!("vertex {x} {y} {z}\n");
                        }
                        stl += "endloop\nendfacet\n";
                    }
                }
            }
        }
        let path = h.dir.join("slab.stl");
        std::fs::write(&path, stl + "endsolid s\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
    }

    /// A block 80 × 60 × 20 with its four top edges rounded R 2, meshed finely (each triangle cut
    /// in four, twice) and inserted as a scan.
    fn rounded_block(h: &mut crate::scenario::Harness) {
        h.app.run("solid.box", json!({"length": 80, "width": 60, "height": 20, "body_name": "Rounded"})).unwrap();
        h.app
            .run(
                "solid.fillet",
                json!({"edges": [[40, 0, 20], [80, 30, 20], [40, 60, 20], [0, 30, 20]], "radius": 2, "tangent_chain": false, "body": "Rounded"}),
            )
            .unwrap();
        let mesh = {
            let state = h.app.session.model.state();
            let body = state.body("Rounded").unwrap();
            body.body.mesh(&solidcraft_engine::kernel::MeshTolerance { chord: 0.005, angle: Some(4f64.to_radians()) }).unwrap()
        };
        let mut triangles: Vec<[solidcore_record::Vec3; 3]> = mesh.triangles.iter().map(|t| t.map(|i| mesh.positions[i as usize])).collect();
        for _ in 0..2 {
            triangles = triangles
                .into_iter()
                .flat_map(|[a, b, c]| {
                    let (ab, bc, ca) = ((a + b) * 0.5, (b + c) * 0.5, (c + a) * 0.5);
                    [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]
                })
                .collect();
        }
        let mut stl = String::from("solid r\n");
        for t in &triangles {
            stl += "facet normal 0 0 0\nouter loop\n";
            for p in t {
                stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
            }
            stl += "endloop\nendfacet\n";
        }
        h.app.run("edit.undo", json!({})).unwrap();
        h.app.run("edit.undo", json!({})).unwrap();
        let path = h.dir.join("rounded.stl");
        std::fs::write(&path, stl + "endsolid r\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
    }

    /// Each step's row has a handle of its own, by its place and its command, and so has
    /// Reconstruct the Rest: clicking one ticks or unticks that row and no other.
    #[test]
    fn each_step_has_a_handle_of_its_own_to_click() {
        let mut h = crate::scenario::Harness::new();
        rounded_block(&mut h);
        h.app.start("scan.reconstruct");
        h.settle();
        h.frames(2);
        let ticked = |h: &crate::scenario::Harness| match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanReconstruct(k)) => (k.ticked.clone(), k.rest),
            _ => panic!("the Reconstruct dialog"),
        };
        assert_eq!(ticked(&h), (vec![true, true], false));
        let handles = ["scan_reconstruct:0:scan.extrude_it", "scan_reconstruct:1:scan.fillet_it", "scan_reconstruct:rest"];
        let places: Vec<[f64; 2]> = handles.iter().map(|name| h.at(&json!({"handle": name})).unwrap_or_else(|e| panic!("{name}: {e}"))).collect();
        assert!(places[0] != places[1] && places[1] != places[2] && places[0] != places[2], "{places:?}");
        h.step(&json!({"click": {"handle": handles[1]}})).unwrap();
        h.frames(2);
        assert_eq!(ticked(&h), (vec![true, false], false), "Fillet It unticked, the rest as they were");
        h.step(&json!({"click": {"handle": handles[2]}})).unwrap();
        h.frames(2);
        assert_eq!(ticked(&h), (vec![true, false], true));
        h.step(&json!({"click": {"handle": handles[0]}})).unwrap();
        h.frames(2);
        assert_eq!(ticked(&h), (vec![false, false], true));
    }

    /// One refused step does not stop the rest: the block with rounded top edges lists Extrude It
    /// and Fillet It; with Fillet It refused, OK still builds the block, closes, and the status
    /// line names the step left out and why. With every step refused, the dialog stays and says so.
    #[test]
    fn ok_builds_the_steps_it_can_and_names_the_one_refused() {
        let mut h = crate::scenario::Harness::new();
        rounded_block(&mut h);
        h.app.start("scan.reconstruct");
        h.settle();
        let found = found(&h.app).expect("read");
        let steps = reconstruct::proposals(&found.reading, false);
        assert!(matches!(steps[..], [Proposal::Extrude(0), Proposal::Fillet]), "{steps:?}");
        let refuse = |refused: &'static [&'static str]| -> solidcraft_engine::CommandCheck {
            std::sync::Arc::new(move |id, _, _| if refused.contains(&id) { Err("the kernel refused it".to_string()) } else { Ok(()) })
        };
        // Everything refused: nothing is built, and the dialog stays open with the refusal.
        h.app.session.set_command_check(Some(refuse(&["scan.extrude_it", "scan.fillet_it"])));
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        let refused = h.app.dialog.as_ref().and_then(|d| d.error.clone());
        assert!(refused.as_ref().is_some_and(|e| e.contains("the kernel refused it")), "{refused:?}");
        assert!(!reconstruct::is_built(&h.app.session, &found.scan));
        // Only the fillet refused: the block is built.
        h.app.session.set_command_check(Some(refuse(&["scan.fillet_it"])));
        let undo = h.app.session.undo.len();
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let built = h.app.session.model.state().bodies.iter().find(|b| b.name.ends_with("Reconstructed")).and_then(|b| b.body.volume_at(0.01));
        assert!(built.is_some_and(|v| (v - 96000.0).abs() < 100.0), "the sharp block: {built:?}");
        assert_eq!(h.app.session.undo.len(), undo + 1, "one undo step for the one step built");
        let said = h.app.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
        assert!(said.contains("1 of 2") && said.contains("Fillet It") && said.contains("the kernel refused it"), "{said}");
    }

    /// Pattern It builds on the cut, hole or extrusion of its own hole or prism, and on nothing
    /// else: left out with that step when OK leaves it out.
    #[test]
    fn a_pattern_builds_on_its_own_cut_or_extrusion() {
        let step = |id: &str, p: Value| (id.to_string(), p);
        let commands = [
            step("scan.extrude_it", json!({"body": "s", "prism": 0})),
            step("scan.pattern_it", json!({"body": "s", "prism": 0})),
            step("scan.extrude_it", json!({"body": "s", "prism": 2})),
            step("scan.cut_it", json!({"body": "s", "hole": 0})),
            step("scan.hole_it", json!({"body": "s", "hole": 1})),
            step("scan.pattern_it", json!({"body": "s", "hole": 1})),
            step("scan.pattern_it", json!({"body": "s", "hole": 0})),
            step("scan.fillet_it", json!({"body": "s"})),
        ];
        assert_eq!(needs(&commands), [None, Some(0), None, None, None, Some(4), Some(3), None]);
    }

    /// A rounded slab as a scan: Reconstruct's button reads it and lists one Extrude It, ticked;
    /// OK builds the slab.
    #[test]
    fn reconstruct_lists_the_prism_and_ok_extrudes_it() {
        let mut h = crate::scenario::Harness::new();
        slab(&mut h);
        h.app.start("scan.reconstruct");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanReconstruct(_))), "the dialog is open");
        let found = found(&h.app).expect("read");
        let steps = reconstruct::proposals(&found.reading, reconstruct::is_built(&h.app.session, &found.scan));
        assert!(matches!(steps.first(), Some(Proposal::Extrude(0))), "{steps:?}");
        let commands = commands(&h.app, &Reconstruct { ticked: vec![true; steps.len()], selected: false, rest: false }).unwrap();
        assert_eq!(commands[0].0, "scan.extrude_it");
        for (id, p) in commands {
            h.app.run(&id, p).unwrap();
        }
        let built = h.app.session.model.state().bodies.iter().find(|b| b.name.ends_with("Reconstructed")).and_then(|b| b.body.volume_at(0.01));
        assert!(built.is_some_and(|v| (v - 3600.0).abs() < 1.0), "{built:?}");
    }

    /// Reconstruct Selection with nothing selected opens nothing and says to select first. With
    /// the slab selected it reads that, lists its steps each taking `selected`, and OK builds it.
    #[test]
    fn reconstruct_selection_reads_what_is_selected() {
        let mut h = crate::scenario::Harness::new();
        slab(&mut h);
        h.app.start("scan.reconstruct_selection");
        assert!(!matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanReconstruct(_))), "nothing to read: no dialog");
        let said = h.app.status.as_ref().map(|s| s.0.clone()).unwrap_or_default();
        assert!(said.contains("Nothing is selected"), "{said}");
        h.app.run("scan.select", json!({"all": true})).unwrap();
        h.app.start("scan.reconstruct_selection");
        assert!(
            matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanReconstruct(k)) if k.selected),
            "the dialog is open, on the selection"
        );
        let found = found(&h.app).expect("read");
        let region = found.selected_region(&h.app.session).expect("the selection is read");
        let steps = reconstruct::proposals(&region.region.reading, reconstruct::is_built(&h.app.session, &found.scan));
        assert!(matches!(steps.first(), Some(Proposal::Extrude(0))), "{steps:?}");
        let commands = commands(&h.app, &Reconstruct { ticked: vec![true; steps.len()], selected: true, rest: false }).unwrap();
        assert!(commands.iter().all(|(_, p)| p["selected"] == true), "{commands:?}");
        for (id, p) in commands {
            h.app.run(&id, p).unwrap();
        }
        let built = h.app.session.model.state().bodies.iter().find(|b| b.name.ends_with("Reconstructed")).and_then(|b| b.body.volume_at(0.01));
        assert!(built.is_some_and(|v| (v - 3600.0).abs() < 1.0), "{built:?}");
    }

    /// Find All can take most of a minute on a big scan: Reconstruct's button opens its dialog at
    /// once, saying the scan is being read, reads it beside the window, and lists the steps when
    /// the reading is in. Nothing waits on the window thread.
    #[test]
    fn reconstruct_reads_the_scan_beside_the_window() {
        let mut h = crate::scenario::Harness::new();
        slab(&mut h);
        h.app.apply_off_thread = Some(std::time::Duration::ZERO);
        h.app.start("scan.reconstruct");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanReconstruct(_))), "the dialog opens at once");
        assert!(reading(&h.app) && found(&h.app).is_none(), "it is reading the scan");
        for _ in 0..500 {
            if !reading(&h.app) {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let found = found(&h.app).expect("read");
        assert!(found.shown, "the scan is coloured by its shapes");
        let steps = reconstruct::proposals(&found.reading, reconstruct::is_built(&h.app.session, &found.scan));
        assert!(matches!(steps.first(), Some(Proposal::Extrude(0))), "{steps:?}");
    }
}
