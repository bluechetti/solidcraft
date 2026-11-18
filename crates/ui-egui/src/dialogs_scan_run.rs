//! The Scan tools that work on a whole scan or surface with nothing to click on it (Remesh,
//! Repair Mesh, Mesh from Points, Combine Scans, Align to Scan, Align All Scans, Align to Model,
//! Close Surface, Make Flat, Refit to Scan, Reconstruct the Rest, Roll onto Scan) open this small
//! dialog instead of running the moment their button is pressed. It names what the tool will work
//! on (picked already: the selected one, else the one the command would take, highlighted and
//! changeable with a click; for Roll onto Scan, the Flat Pattern the design keeps) and shows its
//! few choices; OK runs it. Nothing is guessed out of sight: OK passes what is shown, so what runs
//! is what was seen.
//!
//! The tools that move scans (Align to Scan, Align All Scans, Align to Model, and Combine Scans'
//! laying together) show where OK will put them before OK: the command OK runs is run on a copy of
//! the design beside the window as soon as the dialog names what to move, each scan it moved is
//! painted where it went, and the dialog says how well it fits. Pressing the button and seeing
//! nothing for seconds (Align to Model on the holes plate took 8 s) told nothing.
//!
//! OK keeps what was shown: the preview is worked out as OK works it out (the commands computed
//! whole on a copy of the design), and OK takes that copy when nothing has changed since, so the
//! search is not run a second time (seconds to half a minute on a van's passes) and what lands is
//! exactly what was seen.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

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

/// The tools whose OK moves scans: where they go is worked out and shown before OK. The others show
/// what OK makes or changes, worked out the same way.
const PLACING: &[&str] = &["scan.register", "scan.align_all", "scan.align_to_model", "scan.combine"];

/// The most triangles (of the scans a tool works on) whose result is worked out before OK. A
/// preview does all of OK's work on a copy, so past this it would double the wait and the memory
/// on a vast scan (Combine of 31 million triangles peaked at 30 GB): the dialog says so and OK
/// works it out. Laying scans together is worked out at any size: OK needs the same search.
pub(crate) const PREVIEW_MOST: usize = 2_000_000;

thread_local! {
    /// For tests: a lower [`PREVIEW_MOST`].
    static PREVIEW_CAP: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The most triangles previewed: [`PREVIEW_MOST`], or what a test set.
pub(crate) fn preview_most() -> usize {
    PREVIEW_CAP.with(Cell::get).unwrap_or(PREVIEW_MOST)
}

/// Previews stop past `most` triangles (tests only; `None` puts [`PREVIEW_MOST`] back).
#[cfg(test)]
pub(crate) fn cap_previews(most: Option<usize>) {
    PREVIEW_CAP.with(|c| c.set(most));
}

/// The preview's id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 13;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// What OK would do: what the command replied on a copy of the design, and what it moved, made or
/// changed, painted where it is then.
#[derive(Clone, Debug)]
pub struct Placed {
    reply: Value,
    /// How many bodies it painted (the scans it moved, or what it made or changed).
    moved: usize,
    /// The triangles of the scan it changed, before and after (a mesh it remade or repaired).
    count: Option<(usize, usize)>,
    version: u64,
    triangles: Arc<Vec<u8>>,
    /// The commands it ran, and what they made, for OK to take ([`kept_for_ok`]) while they are
    /// still what OK would run on the design as it is.
    ran: Vec<(String, Value)>,
    computed: Computed,
}

/// What a preview computed, taken once by OK.
#[derive(Clone, Default)]
struct Computed(Arc<std::sync::Mutex<Option<solidcraft_engine::ComputedCommands>>>);

impl std::fmt::Debug for Computed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Computed")
    }
}

/// A preview being worked out, for the values in its key.
type Placing = Arc<crate::scan_job::Job<String, Result<Placed, String>>>;

/// The dialog's values; the bodies it works on are its selection inputs.
#[derive(Clone, Debug)]
pub struct ScanRun {
    /// The command OK runs (one of [`COMMANDS`]).
    pub id: &'static str,
    /// Remesh, Combine Scans: the new mesh's edge length, in mm; `None` keeps the scan's own.
    pub cell: Option<f64>,
    /// Remesh, Combine Scans: sharp edges and corners stay sharp.
    pub sharp: bool,
    /// Remesh: how far off the scan it is made again (positive outward), in mm; `None` on it.
    pub offset: Option<f64>,
    /// Remesh: the offset of the step being edited as it was written (it may name parameters),
    /// sent as it is unless the length is changed here.
    pub offset_expr: Option<String>,
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
    /// The tools that move scans ([`PLACING`]): where OK would put them, for the values in the key,
    /// and the one being worked out.
    placed: Option<(String, Result<Placed, String>)>,
    placing: Option<Placing>,
    /// The tool's result is not worked out before OK: the scans it works on have this many
    /// triangles, past [`PREVIEW_MOST`].
    too_large: Option<usize>,
}

impl ScanRun {
    fn new(id: &'static str) -> ScanRun {
        ScanRun {
            id,
            cell: None,
            sharp: false,
            offset: None,
            offset_expr: None,
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
            placed: None,
            placing: None,
            too_large: None,
        }
    }

    /// The command runs through the whole scan: OK works beside the window.
    pub fn heavy(&self) -> bool {
        !matches!(self.id, "scan.close_surface" | "scan.make_flat")
    }

    /// What OK would do is drawn (where the moved scans go, what is made or changed): the bodies
    /// the dialog names are not lit then, or the model a scan lands on, lit over it, hides where
    /// it lands.
    pub fn shows_placement(&self) -> bool {
        matches!(&self.placed, Some((_, Ok(p))) if p.moved > 0)
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
                // How far off the scan it is made again: out (+) for a liner, in (-) for a socket's fit.
                // A step's offset written as an expression shows what it is now, and stays written
                // so until the length is changed here.
                let written = k.offset_expr.as_deref().and_then(|e| app.session.doc.eval(e, solidcraft_engine::doc::expr::Kind::Length).ok());
                let was = written.or(k.offset).unwrap_or(0.0);
                let mut offset = was;
                form_row(ui, tr("ui.scan_run_offset"), |ui| ui_kit::length_field(ui, &mut offset, &units, decimals));
                if offset != was {
                    k.offset_expr = None;
                }
                k.offset = if k.offset_expr.is_some() { written } else { (offset.is_finite() && offset != 0.0).then_some(offset) };
                // An offset makes the whole scan again: no area to choose.
                if k.offset.is_none() {
                    crate::dialogs_scan_clean::area_row(app, ui, "scan_remesh_area", &mut k.area, k.kept.as_ref());
                    crate::dialogs_scan_clean::mark_area_at(app, &k.area, k.kept.as_ref(), inputs, 1, &mut k.marks);
                }
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
    place(app, k, inputs);
    placed_rows(app, ui, k);
    if let Some(help) = crate::command_help::help(k.id) {
        ui_kit::text(ui, &help, ui_kit::Text::Hint("scan_run_help"));
    }
    false
}

/// The triangles of the scans the dialog names (all its inputs), or of the design's largest scan
/// when it names none (Mesh from Points, a surface tool fitting to a scan).
fn work_size(app: &SolveApp, inputs: &[SelInput]) -> usize {
    let state = app.session.model.state();
    let named: usize = (0..inputs.len())
        .flat_map(|i| items(inputs, i).body_names())
        .filter_map(|n| state.body(&n).and_then(|b| b.body.kept_mesh()).map(|m| m.triangles.len()))
        .sum();
    if named > 0 { named } else { crate::scan_job::biggest_scan(app) }
}

/// What is run on the copy to show what OK does: the command OK runs; for Combine Scans past
/// [`PREVIEW_MOST`] only its laying together (the fusing is not shown). With the flag, the result
/// past the cap is not shown (`too_large`).
fn preview_command(app: &SolveApp, k: &ScanRun, inputs: &[SelInput]) -> (Option<(String, Value)>, Option<usize>) {
    let Some((id, p)) = commands(app, k, inputs).ok().and_then(|c| c.into_iter().next()) else { return (None, None) };
    let size = work_size(app, inputs);
    let large = (size > preview_most()).then_some(size);
    match id.as_str() {
        "scan.combine" if large.is_some() => {
            if !k.align {
                return (None, large);
            }
            let laid = || -> Option<(String, Value)> {
                let base = p.get("body")?.clone();
                let mut bodies = vec![base.clone()];
                bodies.extend(p.get("with")?.as_array()?.iter().cloned());
                Some(("scan.align_all".to_owned(), json!({"bodies": bodies, "hold": [base]})))
            };
            (laid(), large)
        }
        "scan.register" | "scan.align_all" | "scan.align_to_model" => (Some((id, p)), None),
        _ if large.is_some() => (None, large),
        _ => (Some((id, p)), None),
    }
}

/// Work out again what OK would do when the bodies, the choices or the design have changed:
/// beside the window, the last one staying shown until the new one is there.
fn place(app: &SolveApp, k: &mut ScanRun, inputs: &[SelInput]) {
    let (command, too_large) = preview_command(app, k, inputs);
    k.too_large = too_large;
    let Some((id, p)) = command else {
        k.placed = None;
        k.placing = None;
        return;
    };
    let key = format!("{id}|{p}|{}", app.session.revision);
    if k.placed.as_ref().is_some_and(|(shown, _)| *shown == key) {
        k.placing = None;
        return;
    }
    // The colour is read here: the theme is the window thread's.
    let rgb = solidcraft_theme::Tokens::get().analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    if let Some(job) = k.placing.clone() {
        match job.poll() {
            crate::scan_job::Poll::Working if job.key == key => return,
            crate::scan_job::Poll::Done(result) if job.key == key => {
                k.placing = None;
                k.placed = Some((key, result));
                return;
            }
            _ => k.placing = None,
        }
    }
    // Always beside the window: a search over the scan takes a second or more even on a small one.
    let (given, command) = (p.clone(), id.clone());
    if let Some(job) = crate::scan_job::spawn(app, key.clone(), move |s| placed_on_copy(s, &command, &given, rgb)) {
        k.placing = Some(Arc::new(job));
        return;
    }
    k.placed = Some((key, placed_on_copy(&app.session, &id, &p, rgb)));
}

/// `id` run with `p` on a copy of `s`: what it replied, and what it moved (the laying tools), else
/// the body it names in its reply (`body`, `surface`: the scan remade, the mesh or surface made),
/// painted where it is then (past the most a colouring paints, every so many of its triangles).
fn placed_on_copy(s: &solidcraft_engine::Session, id: &str, p: &Value, rgb: [u8; 3]) -> Result<Placed, String> {
    let ran = vec![(id.to_owned(), p.clone())];
    let prepared = s.scratch().compute_commands(&ran, None).map_err(|e| e.to_string())?;
    let reply = prepared.steps().last().map(|(_, _, reply)| reply.clone()).unwrap_or(Value::Null);
    let copy = prepared.computed();
    let laying = PLACING.contains(&id) && id != "scan.combine";
    let named =
        || -> Vec<String> { ["body", "surface"].iter().find_map(|k| reply.get(*k).and_then(Value::as_str)).map(str::to_owned).into_iter().collect() };
    let moved: Vec<String> = if !laying {
        named()
    } else {
        match reply.get("passes").and_then(Value::as_array) {
            Some(passes) => passes
                .iter()
                .filter(|pass| pass.get("moved").and_then(Value::as_bool) == Some(true))
                .filter_map(|pass| pass.get("body").and_then(Value::as_str).map(str::to_owned))
                .collect(),
            None => reply
                .get("moved")
                .and_then(Value::as_bool)
                .filter(|m| *m)
                .and(reply.get("body").and_then(Value::as_str))
                .map(str::to_owned)
                .into_iter()
                .collect(),
        }
    };
    let state = copy.model.state();
    // A scan as triangles; a surface or solid as its display mesh.
    let meshes: Vec<(Vec<solidcore_record::Vec3>, Vec<[u32; 3]>)> = moved
        .iter()
        .filter_map(|name| Some((name, state.body(name)?)))
        .map(|(name, b)| match b.body.is_mesh().then(|| solidcraft_engine::cmd::scan::shown_mesh(copy, name)).flatten() {
            // Where the scan shows (in a placed component, moved with it).
            Some(m) => (m.positions.clone(), m.triangles.clone()),
            None => {
                let m = b.mesh();
                (m.positions.clone(), m.triangles.clone())
            }
        })
        .collect();
    let total: usize = meshes.iter().map(|(_, t)| t.len()).sum();
    let step = total.div_ceil(crate::scan_dots::PAINT_MOST).max(1);
    let mut out = Vec::with_capacity((total / step + 1) * crate::scan_dots::DOT_SIZE);
    for (positions, triangles) in &meshes {
        let lifts = crate::scan_dots::PaintLifts::of(positions, triangles);
        for tri in triangles.iter().step_by(step) {
            lifts.paint(&mut out, positions, *tri, 0.0, rgb);
        }
    }
    // A scan it remade: its triangles before and after.
    let count = (!laying)
        .then(|| moved.first())
        .flatten()
        .and_then(|b| Some((s.model.state().body(b)?.body.kept_mesh()?.triangles.len(), state.body(b)?.body.kept_mesh()?.triangles.len())));
    let version = VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    });
    drop(state);
    Ok(Placed {
        reply,
        moved: moved.len(),
        count,
        version,
        triangles: Arc::new(out),
        ran,
        computed: Computed(Arc::new(std::sync::Mutex::new(Some(prepared)))),
    })
}

/// What the preview computed, for OK to take in place of computing it again: only when it ran
/// exactly the commands OK would run now and the design is as it was then (else `None`, and OK
/// works it out as usual).
pub(crate) fn kept_for_ok(app: &SolveApp, k: &ScanRun, cmds: &[(String, Value)]) -> Option<solidcraft_engine::ComputedCommands> {
    let (_, Ok(placed)) = k.placed.as_ref()? else { return None };
    if placed.ran != cmds {
        return None;
    }
    let mut slot = placed.computed.0.lock().ok()?;
    if !slot.as_ref().is_some_and(|prepared| prepared.is_current(&app.session)) {
        return None;
    }
    slot.take()
}

/// The rows saying where OK would put the scans and how well they fit, or that it is being worked
/// out, or why it cannot be.
fn placed_rows(app: &SolveApp, ui: &mut egui::Ui, k: &ScanRun) {
    let len = |v: f64| crate::prefs::show_mm(app, v, 1);
    let row = |ui: &mut egui::Ui, label: &str, text: &str| form_row(ui, label, |ui| ui_kit::text(ui, text, ui_kit::Text::Cell));
    let num = |r: &Value, key: &str| r.get(key).and_then(Value::as_f64);
    match k.placed.as_ref().map(|(_, r)| r) {
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_run_place_error", true));
        }
        Some(Ok(placed)) => {
            let r = &placed.reply;
            // Combine Scans run whole: its laying together is in `aligned`.
            let laid_reply = r.get("aligned").filter(|a| a.is_object()).unwrap_or(r);
            if let Some(passes) = laid_reply.get("passes").and_then(Value::as_array) {
                let r = laid_reply;
                let laid = passes.iter().filter(|p| p.get("placed").and_then(Value::as_bool) == Some(true)).count();
                let count = solidcraft_ui_text::format("ui.scan_run_laid_count", &[("laid", &laid.to_string()), ("all", &passes.len().to_string())]);
                row(ui, tr("ui.scan_run_laid"), &count);
                let links = r.get("links").and_then(Value::as_array).cloned().unwrap_or_default();
                let worst = links
                    .iter()
                    .filter(|l| l.get("accepted").and_then(Value::as_bool) == Some(true))
                    .filter_map(|l| num(l, "rms_after"))
                    .fold(None, |w: Option<f64>, v| Some(w.map_or(v, |w| w.max(v))));
                if let Some(worst) = worst {
                    row(ui, tr("ui.scan_run_worst_after"), &len(worst));
                }
                // A pairing left out says why only when a pass is left out with it: with every pass
                // laid, another pairing placed it.
                if let Some(note) = links.iter().filter(|_| laid < passes.len()).find_map(|l| l.get("note").and_then(Value::as_str)) {
                    ui_kit::text(ui, note, ui_kit::Text::Report("scan_run_place_note", true));
                }
            } else if PLACING.contains(&k.id) {
                if let Some(v) = num(r, "rms_before") {
                    row(ui, tr("ui.scan_run_off_before"), &len(v));
                }
                if let Some(v) = num(r, "rms_after") {
                    row(ui, tr("ui.scan_run_off_after"), &len(v));
                }
                for key in ["note", "rival"] {
                    if let Some(text) = r.get(key).and_then(Value::as_str) {
                        ui_kit::text(ui, text, ui_kit::Text::Report("scan_run_place_note", true));
                    }
                }
            }
            if !PLACING.contains(&k.id) || r.get("fused").is_some() {
                result_rows(app, ui, k.id, placed);
            }
            // While the next one is worked out, its note takes this line: a line more, then one
            // less when it arrived, moved OK under the pointer (a click aimed at it missed).
            let what = match (k.placing.is_some(), PLACING.contains(&k.id) && r.get("fused").is_none(), placed.moved) {
                (true, ..) => "ui.scan_run_placing",
                (false, true, 0) => "ui.scan_run_stays",
                (false, true, _) => "ui.scan_run_placed_hint",
                (false, false, 0) => "ui.scan_run_nothing_changes",
                (false, false, _) => "ui.scan_run_result_hint",
            };
            let id = if k.placing.is_some() { "scan_run_placing" } else { "scan_run_placed" };
            ui_kit::text(ui, tr(what), ui_kit::Text::Hint(id));
        }
        None => {}
    }
    if let Some(count) = k.too_large {
        let text = solidcraft_ui_text::format("ui.scan_run_too_large", &[("triangles", &count.to_string())]);
        ui_kit::text(ui, &text, ui_kit::Text::Hint("scan_run_too_large"));
    }
    if k.placing.is_some() {
        if !matches!(k.placed.as_ref().map(|(_, r)| r), Some(Ok(_))) {
            ui_kit::text(ui, tr("ui.scan_run_placing"), ui_kit::Text::Hint("scan_run_placing"));
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
}

/// What a tool that makes or changes a body would do, from its reply on the copy: the scan's
/// triangles before and after, and the one or two numbers that say how it went.
fn result_rows(app: &SolveApp, ui: &mut egui::Ui, id: &str, placed: &Placed) {
    let r = &placed.reply;
    let len = |v: f64| crate::prefs::show_mm(app, v, 1);
    let row = |ui: &mut egui::Ui, label: &str, text: &str| form_row(ui, label, |ui| ui_kit::text(ui, text, ui_kit::Text::Cell));
    let int = |r: &Value, key: &str| r.get(key).and_then(Value::as_u64).map(|v| v as usize);
    let part_of =
        |part: usize, all: usize| solidcraft_ui_text::format("ui.scan_run_laid_count", &[("laid", &part.to_string()), ("all", &all.to_string())]);
    let fused = r.get("fused").filter(|f| f.is_object());
    let counted = placed.count.or_else(|| fused.and_then(|f| Some((int(f, "before")?, int(f, "triangles")?))));
    match counted {
        Some((before, after)) if before != after => {
            let change =
                solidcraft_ui_text::format("ui.scan_run_triangles_change", &[("before", &before.to_string()), ("after", &after.to_string())]);
            row(ui, tr("ui.scan_run_triangles"), &change);
        }
        Some((_, after)) => {
            row(ui, tr("ui.scan_run_triangles"), &after.to_string());
        }
        None => {
            if let Some(t) = int(r, "triangles") {
                row(ui, tr("ui.scan_run_triangles"), &t.to_string());
            }
        }
    }
    match id {
        "scan.repair" => {
            // Points written twice a hair apart, joined (the YCB mug's cracks), apart from the faces
            // mended.
            if let Some(joined) = int(r, "joined").filter(|j| *j > 0) {
                row(ui, tr("ui.scan_run_joined"), &joined.to_string());
            }
            let fixed: usize = ["duplicates", "non_manifold", "slivers", "crossing_faces", "refilled"].iter().filter_map(|k| int(r, k)).sum();
            row(ui, tr("ui.scan_run_fixed"), &fixed.to_string());
        }
        "scan.mesh_from_points" => {
            if let (Some(used), Some(all)) = (int(r, "points_used"), int(r, "points")) {
                row(ui, tr("ui.scan_run_points_used"), &part_of(used, all));
            }
            // Passes merged without lying on each other: said, with what was done about it.
            if r.get("warning").is_some_and(|w| !w.is_null())
                && let Some(t) = r.get("thickness").and_then(Value::as_f64)
            {
                row(ui, tr("ui.scan_run_layer"), &len(t));
                ui_kit::text(ui, tr("ui.scan_run_layered"), ui_kit::Text::Hint("scan_run_layered"));
            }
        }
        "scan.remesh" | "scan.combine" => {
            if let Some(v) = fused.unwrap_or(r).get("new_rms").and_then(Value::as_f64) {
                row(ui, tr("ui.scan_run_off_scan"), &len(v));
            }
        }
        "scan.make_flat" => {
            if let Some(v) = r.get("rms").and_then(Value::as_f64) {
                row(ui, tr("ui.scan_run_off_scan"), &len(v));
            }
        }
        "scan.close_surface" => {
            if let Some(v) = int(r, "caps") {
                row(ui, tr("ui.scan_run_closed"), &v.to_string());
            }
            if let Some(v) = int(r, "open") {
                row(ui, tr("ui.scan_run_left_open"), &v.to_string());
            }
        }
        "scan.refit" => {
            if let Some(v) = int(r, "faces") {
                row(ui, tr("ui.scan_run_faces"), &v.to_string());
            }
        }
        _ => {}
    }
}

/// Where OK would put the scans, for the GPU viewport: the moved scans painted there.
pub(crate) fn gpu_preview(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanRun(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let Some((_, Ok(placed))) = k.placed.as_ref() else { return None };
    if placed.triangles.is_empty() {
        return None;
    }
    Some(GpuCloud {
        id: CLOUD_ID,
        version: placed.version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: placed.triangles.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: 3.0 },
    })
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
            let mut p = if k.offset.is_some() || k.offset_expr.is_some() {
                json!({})
            } else {
                crate::dialogs_scan_clean::area_params_at(app, &k.area, k.kept.as_ref(), inputs, 1)?
            };
            p["body"] = json!(need(0, "ui.scan_run_pick_scan")?);
            if let Some(c) = &cell {
                p["cell"] = c.clone();
            }
            p["sharp"] = json!(k.sharp);
            if let Some(e) = &k.offset_expr {
                p["offset"] = json!(e);
            } else if let Some(o) = k.offset {
                p["offset"] = json!(format!("{o} mm"));
            }
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

    /// A 20 x 10 x 10 box scan, `shift` along X, each face cut into a grid of `n` by `n` cells
    /// (twelve triangles leave too little to lay onto a model).
    fn fine_box_scan(h: &mut crate::scenario::Harness, name: &str, shift: f64, n: usize) {
        let path = h.dir.join(format!("{name}.obj"));
        let (size, mut obj, mut count) = ([20.0, 10.0, 10.0], String::new(), 0usize);
        // Each face: the axis it faces along, which side, and its two in-plane axes.
        for (axis, side) in [(0, 0.0), (0, 1.0), (1, 0.0), (1, 1.0), (2, 0.0), (2, 1.0)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            let at = |i: usize, j: usize| {
                let mut p = [0.0; 3];
                p[axis] = side * size[axis];
                p[u] = size[u] * i as f64 / n as f64;
                p[v] = size[v] * j as f64 / n as f64;
                p
            };
            for j in 0..=n {
                for i in 0..=n {
                    let p = at(i, j);
                    obj.push_str(&format!("v {} {} {}\n", p[0] + shift, p[1], p[2]));
                }
            }
            for j in 0..n {
                for i in 0..n {
                    let k = |i: usize, j: usize| count + j * (n + 1) + i + 1;
                    obj.push_str(&format!("f {} {} {}\nf {} {} {}\n", k(i, j), k(i + 1, j), k(i + 1, j + 1), k(i, j), k(i + 1, j + 1), k(i, j + 1)));
                }
            }
            count += (n + 1) * (n + 1);
        }
        std::fs::write(&path, obj).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
    }

    /// Align to Model works out where the scan goes beside the window as soon as it opens (saying
    /// so meanwhile, the design untouched), then says how far off the scan is now and after, paints
    /// it where OK puts it, and stops lighting the two bodies (lit, the model hid where it lands).
    #[test]
    fn align_to_model_shows_where_the_scan_goes_before_ok() {
        let mut h = crate::scenario::Harness::new();
        fine_box_scan(&mut h, "part", 0.0, 16);
        // The model where the scan is (wherever inserting put it), 3 mm along X from it.
        let lo = crate::viewport::model_bounds(&h.app).min;
        h.app
            .run(
                "solid.box",
                json!({"body_name": "Model", "operation": "new", "length": 20, "width": 10, "height": 10, "corner": [lo.x - 3.0, lo.y, lo.z]}),
            )
            .unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        let features = h.app.session.doc.features.len();
        let gate = crate::scan_job::hold();
        h.app.start("scan.align_to_model");
        h.frames(2);
        let working = matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanRun(k)) if k.placing.is_some() && k.placed.is_none());
        gate.open();
        for _ in 0..1000 {
            h.frames(1);
            if matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanRun(k)) if k.placed.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(working, "worked out beside the window, saying so");
        assert_eq!(h.app.session.doc.features.len(), features, "nothing in the design moves before OK");
        let Some(d) = h.app.dialog.as_ref() else { panic!("the dialog stays open") };
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        let Some((_, Ok(placed))) = &k.placed else { panic!("where it goes: {:?}", k.placed.as_ref().map(|(_, r)| r.as_ref().err())) };
        let (before, after) = (placed.reply["rms_before"].as_f64().unwrap(), placed.reply["rms_after"].as_f64().unwrap());
        assert!(before > 0.5 && after < 0.1 * before, "3 mm off, then on it: {before} -> {after}");
        assert!(k.shows_placement() && placed.moved == 1);
        assert!(super::gpu_preview(&h.app).is_some_and(|c| !c.triangles.is_empty()), "painted where OK puts it");
        assert!(h.app.highlighted().iter().all(|x| !matches!(x, solidcraft_engine::Sel::Body { .. })), "the two bodies are not lit over it");
        // OK takes what the preview computed instead of searching again, and lands where shown.
        let kept = placed.computed.clone();
        let shown = placed.reply["rms_after"].as_f64().unwrap();
        assert!(kept.0.lock().unwrap().is_some());
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(kept.0.lock().unwrap().is_none(), "OK took the preview's result");
        assert_eq!(h.app.session.doc.features.len(), features + 1, "one Move");
        let check = h.app.session.scratch().execute("scan.align_to_model", &json!({"body": "part", "model": "Model", "search": false})).unwrap();
        assert!(check["rms_before"].as_f64().unwrap() <= shown + 1e-6, "the scan lies where the preview put it: {check} vs {shown}");
    }

    /// An offset chosen in Remesh goes to the command, the whole scan (no area): OK makes the scan
    /// again that far off it, the step keeps it, and editing the step brings it back.
    #[test]
    fn remesh_takes_an_offset_for_the_whole_scan() {
        let mut h = crate::scenario::Harness::new();
        fine_box_scan(&mut h, "part", 0.0, 8);
        h.app.start("scan.remesh");
        if let Some(Kind::ScanRun(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.cell = Some(1.0);
            k.offset = Some(-1.5);
        }
        let d = h.app.dialog.as_ref().unwrap();
        let Kind::ScanRun(k) = &d.kind else { panic!() };
        let sent = super::commands(&h.app, k, &d.inputs).unwrap();
        let (id, p) = &sent[0];
        assert_eq!(id, "scan.remesh");
        assert_eq!(p["offset"], json!("-1.5 mm"), "{p}");
        assert!(p.get("area").is_none() && p.get("point").is_none() && p.get("saved").is_none(), "the whole scan: {p}");
        let mut refilled = super::ScanRun::new("scan.remesh");
        let step = solidcraft_engine::doc::scan_clean::CleanStep::Remesh {
            cell: Some(1.0),
            sharp: false,
            offset: Some("-gap".into()),
            with: Vec::new(),
            area: None,
        };
        let mut kind = Kind::ScanRun(refilled.clone());
        crate::dialogs_scan_clean::refill(&mut kind, &step);
        if let Kind::ScanRun(k) = kind {
            refilled = k;
        }
        assert_eq!(refilled.offset_expr.as_deref(), Some("-gap"), "editing the step keeps its offset as written");
        let mut sent_again = refilled.clone();
        sent_again.cell = Some(1.0);
        let (_, p) = &super::commands(&h.app, &sent_again, &d.inputs).unwrap()[0];
        assert_eq!(p["offset"], json!("-gap"), "and sends it back as written: {p}");
    }

    /// The tools that make or change a body show it before OK too: Remesh works out the new mesh on
    /// a copy beside the window and says its triangles before and after; past the cap it says the
    /// scan is too large to preview and works nothing out (OK still runs it).
    #[test]
    fn remesh_shows_what_it_makes_before_ok_up_to_the_cap() {
        let mut h = crate::scenario::Harness::new();
        fine_box_scan(&mut h, "part", 0.0, 8);
        let triangles = 6 * 8 * 8 * 2;
        let wait = |h: &mut crate::scenario::Harness| {
            for _ in 0..1000 {
                h.frames(1);
                if matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanRun(k)) if k.placed.is_some() || k.too_large.is_some()) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        let features = h.app.session.doc.features.len();
        h.app.start("scan.remesh");
        if let Some(Kind::ScanRun(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.cell = Some(1.0);
        }
        wait(&mut h);
        assert_eq!(h.app.session.doc.features.len(), features, "nothing changes before OK");
        let Some(Kind::ScanRun(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!() };
        let Some((_, Ok(placed))) = &k.placed else { panic!("the remeshed scan: {:?}", k.placed.as_ref().map(|(_, r)| r.as_ref().err())) };
        let (before, after) = placed.count.expect("its triangles before and after");
        assert_eq!(before, triangles);
        assert!(after > before, "1 mm cells on a 20 mm box make more triangles: {after}");
        assert!(super::gpu_preview(&h.app).is_some(), "what OK makes is painted");
        h.app.dialog = None;
        super::cap_previews(Some(triangles - 1));
        h.app.start("scan.remesh");
        wait(&mut h);
        super::cap_previews(None);
        let Some(Kind::ScanRun(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!() };
        assert_eq!(k.too_large, Some(triangles), "past the cap: said, not worked out");
        assert!(k.placed.is_none() && k.placing.is_none());
        assert!(super::commands(&h.app, k, &h.app.dialog.as_ref().unwrap().inputs).is_ok(), "OK still runs it");
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
