//! The Scan tools that work on a whole scan or surface with nothing to click on it (Remesh,
//! Repair Mesh, Mesh from Points, Combine Scans, Align to Scan, Align All Scans, Align to Model,
//! Close Surface, Make Flat, Refit to Scan, Reconstruct the Rest, Roll onto Scan) open this small
//! dialog instead of running the moment their button is pressed. It names what the tool will work
//! on (picked already: the selected one, else the one the command would take, highlighted and
//! changeable with a click; for Roll onto Scan, the Flat Pattern the design keeps) and shows its
//! few choices; OK runs it. Nothing is guessed out of sight: OK passes what is shown, so what runs
//! is what was seen.

use serde_json::{Value, json};
use solidcraft_engine::Sel;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{BODIES, SelInput, SelectionExt as _, items};
use crate::ui_kit::{self, choice_row};

/// The commands that open this dialog.
pub const COMMANDS: &[&str] = &[
    "scan.remesh",
    "scan.repair",
    "scan.mesh_from_points",
    "scan.combine",
    "scan.register",
    "scan.align_all",
    "scan.align_to_model",
    "scan.close_surface",
    "scan.make_flat",
    "scan.refit",
    "scan.reconstruct_rest",
    "scan.roll_onto_scan",
];

/// The dialog's values; the bodies it works on are its selection inputs.
#[derive(Clone, Debug)]
pub struct ScanRun {
    /// The command OK runs (one of [`COMMANDS`]).
    pub id: &'static str,
    /// Remesh, Combine Scans: the new mesh's edge length, in mm; `None` keeps the scan's own.
    pub cell: Option<f64>,
    /// Remesh, Combine Scans: sharp edges and corners stay sharp.
    pub sharp: bool,
    /// Repair Mesh: faces passing through each other are taken off and the holes closed.
    pub crossings: bool,
    /// Mesh from Points: 0 closes the surface over sparse places, 1 leaves them open.
    pub sparse: usize,
    /// Combine Scans: the passes are laid together before they are fused.
    pub align: bool,
    /// Close Surface: 0 the patches touch the faces round them, 1 they meet them smoothly.
    pub meet: usize,
    /// Refit to Scan: the hand edits are dropped and the surface fitted afresh.
    pub discard: bool,
    /// Reconstruct the Rest: the most pieces it tries.
    pub rounds: usize,
    /// Roll onto Scan: the Flat Patterns the design keeps (sketch id and name), and the one chosen.
    pub patterns: Vec<(u64, String)>,
    pub pattern: usize,
    /// Remesh: which part of the scan (the whole scan unless another way is chosen; the click is
    /// the second input), the area of the step being edited, and the area marked on the scan.
    pub area: crate::scan_area::Area,
    pub kept: Option<Value>,
    pub marks: crate::dialogs_scan_clean::AreaMarks,
}

impl ScanRun {
    fn new(id: &'static str) -> ScanRun {
        ScanRun {
            id,
            cell: None,
            sharp: false,
            crossings: true,
            sparse: 1,
            align: true,
            meet: 0,
            discard: false,
            rounds: 6,
            patterns: Vec::new(),
            pattern: 0,
            area: crate::scan_area::Area::clean(),
            kept: None,
            marks: crate::dialogs_scan_clean::AreaMarks::default(),
        }
    }

    /// The command runs through the whole scan: OK works beside the window.
    pub fn heavy(&self) -> bool {
        !matches!(self.id, "scan.close_surface" | "scan.make_flat")
    }

    pub fn title(&self) -> &'static str {
        match self.id {
            "scan.remesh" => "REMESH",
            "scan.repair" => "REPAIR MESH",
            "scan.mesh_from_points" => "MESH FROM POINTS",
            "scan.combine" => "COMBINE SCANS",
            "scan.register" => "ALIGN TO SCAN",
            "scan.align_all" => "ALIGN ALL SCANS",
            "scan.align_to_model" => "ALIGN TO MODEL",
            "scan.close_surface" => "CLOSE SURFACE",
            "scan.make_flat" => "MAKE FLAT",
            "scan.refit" => "REFIT TO SCAN",
            "scan.roll_onto_scan" => "ROLL ONTO SCAN",
            _ => "RECONSTRUCT THE REST",
        }
    }
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The scans in the design, in the order they were made.
fn scans(app: &SolveApp) -> Vec<String> {
    app.session.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect()
}

/// The bodies that are not scans (solids and surfaces), in the order they were made.
fn models(app: &SolveApp) -> Vec<String> {
    app.session.model.state().bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.clone()).collect()
}

/// The bodies picked before the button, each once, in the order picked.
fn selected(app: &SolveApp) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in &app.session.selection {
        if let Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } = s
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    }
    out
}

/// An input holding `names` to start with.
fn input(label: &'static str, multi: bool, names: impl IntoIterator<Item = String>) -> SelInput {
    let mut i = SelInput::new(label, BODIES, multi);
    i.items = names.into_iter().map(|name| Sel::Body { name }).collect();
    i
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let id = *COMMANDS.iter().find(|c| **c == id)?;
    let s = &app.session;
    let all = scans(app);
    let picked = selected(app);
    let picked_scans: Vec<String> = picked.iter().filter(|n| all.contains(n)).cloned().collect();
    let the_scan = || solidcraft_engine::cmd::scan::mesh_body_name(s, None, id).ok();
    let the_surface = || solidcraft_engine::cmd::scan::slice::slice_target(s, None, id).ok();
    let inputs = match id {
        // Remesh's second input: a click on the scan, for its area.
        "scan.remesh" => vec![input("Body", false, the_scan()), SelInput::new("Point", crate::selection::FACES, false)],
        "scan.repair" | "scan.reconstruct_rest" => vec![input("Body", false, the_scan())],
        // The cloud is the design's only one; there is nothing to choose yet.
        "scan.mesh_from_points" => Vec::new(),
        "scan.combine" | "scan.align_all" => {
            vec![input("Bodies", true, if picked_scans.len() >= 2 { picked_scans } else { all })]
        }
        "scan.register" => {
            // The scan moved and the one it is laid on: the two picked, else with two scans the
            // one made last onto the other.
            let (moved, onto) = match (picked_scans.as_slice(), all.as_slice()) {
                ([a, b, ..], _) => (Some(a.clone()), Some(b.clone())),
                ([a], _) => (Some(a.clone()), all.iter().find(|n| *n != a).filter(|_| all.len() == 2).cloned()),
                ([], [a, b]) => (Some(b.clone()), Some(a.clone())),
                _ => (None, None),
            };
            vec![input("Body", false, moved), input("Target", false, onto)]
        }
        // The patterns are a choice of their own: nothing to click.
        "scan.roll_onto_scan" => Vec::new(),
        "scan.align_to_model" => {
            let others = models(app);
            let model = picked.iter().find(|n| others.contains(n)).cloned().or_else(|| (others.len() == 1).then(|| others[0].clone()));
            vec![input("Body", false, the_scan()), input("Target", false, model)]
        }
        _ => vec![input("Surface", false, the_surface())],
    };
    let mut k = ScanRun::new(id);
    if id == "scan.roll_onto_scan" {
        // Each Flat Pattern whose sketch is still there; the one being edited, else the last, first chosen.
        k.patterns = s.doc.scan_flat_patterns.iter().filter_map(|p| s.doc.feature(p.sketch).map(|f| (p.sketch, f.name.clone()))).collect();
        k.pattern = s.active_sketch.and_then(|a| k.patterns.iter().position(|(id, _)| *id == a)).unwrap_or(k.patterns.len().saturating_sub(1));
    }
    Some((Kind::ScanRun(k), inputs))
}

/// The dialog's rows (after its inputs).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut ScanRun, inputs: &[SelInput]) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    match k.id {
        "scan.remesh" | "scan.combine" => {
            if k.id == "scan.combine" {
                form_row(ui, tr("ui.scan_run_align"), |ui| check(ui, &mut k.align));
            }
            let mut own = k.cell.is_none();
            form_row(ui, tr("ui.scan_run_own_edge"), |ui| check(ui, &mut own));
            if own {
                k.cell = None;
            } else {
                let mut cell = k.cell.unwrap_or(1.0);
                form_row(ui, tr("ui.scan_run_edge"), |ui| ui_kit::length_field(ui, &mut cell, &units, decimals));
                k.cell = Some(cell.max(0.001));
            }
            form_row(ui, tr("ui.scan_run_sharp"), |ui| check(ui, &mut k.sharp));
            if k.id == "scan.remesh" {
                crate::dialogs_scan_clean::area_row(app, ui, "scan_remesh_area", &mut k.area, k.kept.as_ref());
                crate::dialogs_scan_clean::mark_area_at(app, &k.area, k.kept.as_ref(), inputs, 1, &mut k.marks);
            }
        }
        "scan.repair" => {
            form_row(ui, tr("ui.scan_run_crossings"), |ui| check(ui, &mut k.crossings));
        }
        "scan.mesh_from_points" => {
            let options = [tr("ui.scan_run_sparse_close"), tr("ui.scan_run_sparse_open")];
            choice_row(ui, tr("ui.scan_run_sparse"), "scan_run_sparse", &mut k.sparse, &options);
        }
        "scan.close_surface" => {
            let options = [tr("ui.scan_run_meet_touch"), tr("ui.scan_run_meet_smooth")];
            choice_row(ui, tr("ui.scan_run_meet"), "scan_run_meet", &mut k.meet, &options);
        }
        "scan.refit" => {
            form_row(ui, tr("ui.scan_run_discard"), |ui| check(ui, &mut k.discard));
        }
        "scan.reconstruct_rest" => {
            form_row(ui, tr("ui.scan_run_rounds"), |ui| ui_kit::number_field(ui, &mut k.rounds, 1..=20, 0.2, ""));
        }
        "scan.roll_onto_scan" => {
            if k.patterns.is_empty() {
                ui_kit::text(ui, tr("ui.scan_run_no_pattern"), ui_kit::Text::Report("scan_run_no_pattern", true));
            } else {
                let names: Vec<&str> = k.patterns.iter().map(|(_, n)| n.as_str()).collect();
                choice_row(ui, tr("ui.scan_run_pattern"), "scan_run_pattern", &mut k.pattern, &names);
            }
        }
        _ => {}
    }
    if let Some(help) = crate::command_help::help(k.id) {
        ui_kit::text(ui, &help, ui_kit::Text::Hint("scan_run_help"));
    }
    false
}

/// The one body of input `i`.
fn one(inputs: &[SelInput], i: usize) -> Option<String> {
    items(inputs, i).body_names().into_iter().next()
}

/// What OK runs: the command on the bodies shown, with the choices made.
pub fn commands(app: &SolveApp, k: &ScanRun, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let need = |i: usize, why: &'static str| one(inputs, i).ok_or_else(|| tr(why).to_owned());
    let cell = k.cell.map(|c| json!(format!("{c} mm")));
    let p = match k.id {
        "scan.remesh" => {
            let mut p = crate::dialogs_scan_clean::area_params_at(app, &k.area, k.kept.as_ref(), inputs, 1)?;
            p["body"] = json!(need(0, "ui.scan_run_pick_scan")?);
            if let Some(c) = &cell {
                p["cell"] = c.clone();
            }
            p["sharp"] = json!(k.sharp);
            p
        }
        "scan.repair" => json!({"body": need(0, "ui.scan_run_pick_scan")?, "crossings": k.crossings}),
        "scan.reconstruct_rest" => json!({"body": need(0, "ui.scan_run_pick_scan")?, "rounds": k.rounds}),
        "scan.mesh_from_points" => json!({"trim": if k.sparse == 0 { 0.0 } else { 0.3 }}),
        "scan.combine" | "scan.align_all" => {
            let names = items(inputs, 0).body_names();
            if names.len() < 2 {
                return Err(tr("ui.scan_run_pick_two").to_owned());
            }
            if k.id == "scan.align_all" {
                json!({"bodies": names})
            } else {
                json!({"body": names[0], "with": names[1..], "align": k.align, "cell": cell, "sharp": k.sharp})
            }
        }
        "scan.register" | "scan.align_to_model" => {
            let body = need(0, "ui.scan_run_pick_scan")?;
            let target = need(1, "ui.scan_run_pick_target")?;
            if body == target {
                return Err(tr("ui.scan_run_pick_target").to_owned());
            }
            if k.id == "scan.register" { json!({"body": body, "to": target}) } else { json!({"body": body, "model": target}) }
        }
        "scan.close_surface" => {
            json!({"surface": need(0, "ui.scan_run_pick_surface")?, "continuity": if k.meet == 1 { "g1" } else { "g0" }})
        }
        "scan.make_flat" => json!({"surface": need(0, "ui.scan_run_pick_surface")?}),
        "scan.refit" => json!({"surface": need(0, "ui.scan_run_pick_surface")?, "discard_edits": k.discard}),
        "scan.roll_onto_scan" => {
            let (pattern, _) = k.patterns.get(k.pattern).ok_or_else(|| tr("ui.scan_run_no_pattern").to_owned())?;
            json!({"pattern": pattern})
        }
        other => return Err(format!("{other} has no dialog here")),
    };
    // Unset choices are left for the command's own default.
    let p = match p {
        Value::Object(mut m) => {
            m.retain(|_, v| !v.is_null());
            Value::Object(m)
        }
        p => p,
    };
    Ok(vec![(k.id.to_owned(), p)])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::dialogs::Kind;

    fn box_scan(h: &mut crate::scenario::Harness, name: &str, shift: f64) {
        let path = h.dir.join(format!("{name}.obj"));
        let mut obj = String::new();
        for z in [0.0, 10.0] {
            for (x, y) in [(0.0, 0.0), (20.0, 0.0), (20.0, 10.0), (0.0, 10.0)] {
                obj.push_str(&format!("v {} {y} {z}\n", x + shift));
            }
        }
        for f in ["1 3 2", "1 4 3", "5 6 7", "5 7 8", "1 2 6", "1 6 5", "2 3 7", "2 7 6", "3 4 8", "3 8 7", "4 1 5", "4 5 8"] {
            obj.push_str(&format!("f {f}\n"));
        }
        std::fs::write(&path, obj).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
    }

    /// Repair Mesh opens a dialog naming the scan instead of running; OK runs it on that scan.
    #[test]
    fn a_whole_scan_tool_opens_a_dialog_naming_the_scan_and_ok_runs_it() {
        let mut h = crate::scenario::Harness::new();
        box_scan(&mut h, "part", 0.0);
        let features = h.app.session.doc.features.len();
        h.app.start("scan.repair");
        let Some(d) = h.app.dialog.as_ref() else { panic!("Repair Mesh opens a dialog") };
        let Kind::ScanRun(k) = &d.kind else { panic!("the scan tool dialog") };
        assert_eq!(h.app.session.doc.features.len(), features, "nothing runs before OK");
        let body = h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
        let cmds = super::commands(&h.app, k, &d.inputs).unwrap();
        assert_eq!(cmds[0].0, "scan.repair");
        assert_eq!(cmds[0].1["body"], json!(body), "the scan shown is the one OK works on");
    }

    /// Remesh opens on the whole scan; Round the click sends the click (its second input) and the
    /// radius, and OK keeps the area in the step.
    #[test]
    fn remesh_takes_an_area_round_a_click() {
        let mut h = crate::scenario::Harness::new();
        box_scan(&mut h, "part", 0.0);
        let body = h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
        h.app.start("scan.remesh");
        let Some(d) = h.app.dialog.as_ref() else { panic!("Remesh opens a dialog") };
        let Kind::ScanRun(k) = &d.kind else { panic!("the scan tool dialog") };
        let whole = super::commands(&h.app, k, &d.inputs).unwrap();
        assert!(whole[0].1.get("point").is_none() && whole[0].1.get("area").is_none(), "the whole scan, as before: {}", whole[0].1);
        let bounds = crate::viewport::model_bounds(&h.app);
        let (lo, hi) = (bounds.min, bounds.max);
        let top = solidcore_record::Vec3::new((lo.x + hi.x) / 2.0, (lo.y + hi.y) / 2.0, hi.z);
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[1].items.push(solidcraft_engine::Sel::Face { body: body.clone(), index: 0, point: top });
            if let Kind::ScanRun(k) = &mut d.kind {
                k.area.way = crate::scan_area::ROUND;
                k.area.radius = 0.3 * (hi.x - lo.x);
            }
        }
        let Some(d) = h.app.dialog.as_ref() else { panic!() };
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        let sent = super::commands(&h.app, k, &d.inputs).unwrap();
        assert!(sent[0].1.get("point").is_some() && sent[0].1.get("radius").is_some(), "{}", sent[0].1);
        h.app.run(&sent[0].0, sent[0].1.clone()).unwrap();
        let step = h.app.session.doc.features.iter().rev().find_map(|f| match &f.kind {
            solidcraft_engine::doc::FeatureKind::ScanClean { step, .. } => Some(step.clone()),
            _ => None,
        });
        assert!(matches!(step, Some(solidcraft_engine::doc::scan_clean::CleanStep::Remesh { area: Some(_), .. })), "{step:?}");
    }

    /// With two scans, Align to Scan starts with the one made last laid onto the other, and
    /// Combine Scans with both; a surface tool with no surface says to pick one.
    #[test]
    fn the_bodies_a_tool_would_take_are_filled_in_and_shown() {
        let mut h = crate::scenario::Harness::new();
        box_scan(&mut h, "first", 0.0);
        box_scan(&mut h, "second", 1.0);
        let names: Vec<String> = h.app.session.model.state().bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.clone()).collect();
        assert_eq!(names.len(), 2);
        h.app.start("scan.register");
        let d = h.app.dialog.take().unwrap();
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        let cmds = super::commands(&h.app, k, &d.inputs).unwrap();
        assert_eq!((cmds[0].1["body"].clone(), cmds[0].1["to"].clone()), (json!(names[1]), json!(names[0])));
        h.app.start("scan.combine");
        let d = h.app.dialog.take().unwrap();
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        let cmds = super::commands(&h.app, k, &d.inputs).unwrap();
        assert_eq!(cmds[0].1["with"].as_array().map(Vec::len), Some(1));
        h.app.start("scan.make_flat");
        let d = h.app.dialog.take().unwrap();
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        assert!(super::commands(&h.app, k, &d.inputs).is_err(), "no surface yet: OK waits for one");
        // No Flat Pattern in the design: Roll onto Scan says to flatten first, OK runs nothing.
        h.app.start("scan.roll_onto_scan");
        let d = h.app.dialog.take().unwrap();
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        assert!(k.patterns.is_empty() && super::commands(&h.app, k, &d.inputs).is_err());
    }
}
