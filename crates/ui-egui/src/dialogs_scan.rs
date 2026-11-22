//! Stand on Base (`scan.stand_on_base`): the dialog finds the ways the scan could stand and marks
//! the steadiest on it, where the part would touch the ground. The Base list chooses another; a
//! face clicked on the scan is grown from the click and marked instead, and three or more clicks
//! (feet, rails, a rim) are the plane through them. OK stands the scan on it.
//!
//! The marks are the scan's points drawn as dots in the 3D pass through the point-cloud path, in
//! the analysis yellow, built again only when the choice changes. Symmetry's colouring by mismatch
//! goes the same way.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::{BasePreview, base_preview, mismatch_preview, scan_box, symmetry_preview};
use solidcraft_engine::schema::scan::ScanMirror;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{FACES, PLANES, SelInput};
use crate::ui_kit::{self, choice_row};

/// The marks' id among the clouds the viewport draws (the deviation map is `u64::MAX - 1`), and
/// Symmetry's colouring by mismatch.
const CLOUD_ID: u64 = u64::MAX - 2;
const MISMATCH_ID: u64 = u64::MAX - 3;
/// What Complete by Mirroring would add, shown before OK.
const FILL_ID: u64 = u64::MAX - 24;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Stand on Base dialog's values (the clicked face is its selection input).
#[derive(Clone, Debug, Default)]
pub struct StandBase {
    /// The base chosen in the list, used while no face is picked.
    pub index: usize,
    /// What is marked now, and what it was worked out for.
    shown: Option<Shown>,
}

#[derive(Clone, Debug)]
struct Shown {
    key: Key,
    result: Result<Preview, String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    points: Vec<[u64; 3]>,
    index: usize,
    revision: u64,
}

#[derive(Clone, Debug)]
struct Preview {
    found: BasePreview,
    version: u64,
    dots: Arc<Vec<u8>>,
}

/// The dialogs for Stand on Base, and for Find Symmetry and Square Up (one dialog, its OK set to
/// keep the plane or to square the scan to it).
pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    match id {
        // Pressed again while the scan stands where it was put, the next base is offered.
        "scan.stand_on_base" => {
            let index = solidcraft_engine::cmd::scan::next_base(&app.session, None);
            Some((Kind::ScanBase(StandBase { index, ..StandBase::default() }), vec![SelInput::new("Face", FACES, true).places()]))
        }
        "scan.symmetry" | "scan.square_to_symmetry" | "scan.complete_by_mirroring" => {
            let square = match id {
                "scan.square_to_symmetry" => SQUARE,
                "scan.complete_by_mirroring" => COMPLETE,
                _ => KEEP,
            };
            // Completing is a guess that the two sides are alike: the colouring shows whether they are.
            // A plane picked is the mirror plane, used as it is.
            Some((
                Kind::ScanSymmetry(Symmetry { square, colour: square == COMPLETE, ..Symmetry::default() }),
                vec![SelInput::new("Plane", PLANES, false)],
            ))
        }
        _ => None,
    }
}

fn tr(text: &str) -> &str {
    solidcraft_ui_text::tr_label(text)
}

/// The clicks on the scan: the scan (the first click's) and the points on it.
fn picked(inputs: &[SelInput]) -> Option<(&str, Vec<Vec3>)> {
    let faces: Vec<(&str, Vec3)> = crate::selection::items(inputs, 0)
        .iter()
        .filter_map(|s| match s {
            Sel::Face { body, point, .. } => Some((body.as_str(), *point)),
            _ => None,
        })
        .collect();
    let body = faces.first()?.0;
    Some((body, faces.iter().filter(|(b, _)| *b == body).map(|(_, p)| *p).collect()))
}

/// Work out what to mark, when the choice or the design has changed since it last was.
fn refresh(app: &SolveApp, k: &mut StandBase, inputs: &[SelInput]) {
    let pick = picked(inputs);
    let points = pick.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
    let key = Key {
        points: points.iter().map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect(),
        index: k.index,
        revision: app.session.revision,
    };
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = base_preview(&app.session, pick.as_ref().map(|(b, _)| *b), &points, k.index).map_err(|e| e.to_string()).map(|found| {
        // Yellow, the analysis colours' middle one: the accent blue is what a whole selected body
        // turns, so a base marked in it could not be told apart.
        let rgb = solidcraft_theme::Tokens::get().analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut dots = Vec::with_capacity(found.marked.len() * crate::scan_dots::DOT_SIZE);
        for at in &found.marked {
            cloud_dot(&mut dots, &Dot { at: *at, rgb, ..Dot::default() });
        }
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Preview { found, version, dots: Arc::new(dots) }
    });
    k.shown = Some(Shown { key, result });
}

/// The dialog's own rows (after the face input); true when Enter was pressed in a value.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut StandBase, inputs: &[SelInput]) -> bool {
    refresh(app, k, inputs);
    let picked = picked(inputs).is_some();
    let Some(shown) = &k.shown else { return false };
    match &shown.result {
        Err(why) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_base_error", true));
        }
        Ok(p) => {
            let base = &p.found.base;
            if !picked && p.found.count > 0 {
                let labels: Vec<String> =
                    (0..p.found.count).map(|i| format!("{} {}", solidcraft_ui_text::tr("ui.scan_base_number"), i + 1)).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                let mut index = k.index % p.found.count;
                choice_row(ui, solidcraft_ui_text::tr("ui.scan_base"), "scan_base", &mut index, &labels);
                k.index = index;
            }
            let what = match base.kind.as_str() {
                "flat" => solidcraft_ui_text::tr("ui.scan_base_flat"),
                "resting" => solidcraft_ui_text::tr("ui.scan_base_resting"),
                "axis" => solidcraft_ui_text::tr("ui.scan_base_axis"),
                _ => solidcraft_ui_text::tr("ui.scan_base_grown"),
            };
            form_row(ui, solidcraft_ui_text::tr("ui.scan_base_kind"), |ui| ui_kit::text(ui, what, ui_kit::Text::Cell));
            if base.area > 0.0 {
                let area = crate::prefs::show_mm(app, base.area, 2);
                form_row(ui, tr("Area"), |ui| ui_kit::text(ui, &area, ui_kit::Text::Cell));
            }
            if base.tip_degrees > 0.0 {
                let tip = format!("{:.0}°", base.tip_degrees);
                form_row(ui, solidcraft_ui_text::tr("ui.scan_base_tips"), |ui| ui_kit::text(ui, &tip, ui_kit::Text::Cell));
            }
            ui_kit::text(ui, solidcraft_ui_text::tr("ui.scan_base_hint"), ui_kit::Text::Hint("scan_base_hint"));
        }
    }
    false
}

/// The command the dialog's OK runs: stand on the face clicked, or on the base chosen.
pub fn commands(k: &StandBase, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let shown = k.shown.as_ref().ok_or("still finding the bases")?;
    let p = shown.result.as_ref().map_err(Clone::clone)?;
    let mut params = Map::new();
    params.insert("body".into(), json!(p.found.body));
    match picked(inputs).map(|(_, p)| p).unwrap_or_default().as_slice() {
        [] => params.insert("base".into(), json!(p.found.index.unwrap_or(0))),
        [point] => params.insert("point".into(), json!([point.x, point.y, point.z])),
        points => params.insert("points".into(), json!(points.iter().map(|q| [q.x, q.y, q.z]).collect::<Vec<_>>())),
    };
    Ok(vec![("scan.stand_on_base".into(), Value::Object(params))])
}

/// The marks for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanBase(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let p = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: p.version,
        dots: p.dots.clone(),
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// The Symmetry dialog (Find Symmetry, Square Up): the mirror plane found is drawn as a sheet over
/// the scan, clipped to the scan's box. With "Draw a guide" on, the next drag in the view draws a
/// line along where the plane runs (free, in the air, its ends at the depth of the scan's middle):
/// the plane through it and the way it was seen from is searched near, as SurfaceCraft's Sketch
/// Guide does, and the view is for turning again. OK keeps the plane or squares the scan to it.
#[derive(Clone, Debug, Default)]
pub struct Symmetry {
    /// What OK does: [`KEEP`] the plane, [`SQUARE`] (make it X = 0) or [`COMPLETE`] by mirroring.
    pub square: usize,
    /// The scan is coloured by how far each point's reflection lands from it.
    pub colour: bool,
    colouring: Option<Colouring>,
    /// With On OK set to complete: what OK would add, for the plane shown.
    filling: Option<Filling>,
    /// The next drag in the view draws a guide.
    pub drawing: bool,
    /// The guide drawn: from, to, and the way it was seen from (scan toward the eye).
    guide: Option<[Vec3; 3]>,
    /// A guide being dragged, on the screen.
    dragging: Option<(egui::Pos2, egui::Pos2)>,
    found: Option<Found>,
}

#[derive(Clone, Debug)]
struct Found {
    key: (Option<[[u64; 3]; 3]>, u64, Option<[[u64; 3]; 2]>, bool),
    body: Option<(String, Vec3, Vec3)>,
    mirror: Result<ScanMirror, String>,
    /// The plane picked and used as it is, by name.
    plane: Option<String>,
}

fn bits(v: Vec3) -> [u64; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

/// What Symmetry's OK does: keep the plane, make it X = 0, or complete the scan by mirroring.
pub const KEEP: usize = 0;
pub const SQUARE: usize = 1;
pub const COMPLETE: usize = 2;

/// The command Symmetry's OK runs.
pub fn symmetry_command(k: &Symmetry) -> &'static str {
    match k.square {
        SQUARE => "scan.square_to_symmetry",
        COMPLETE => "scan.complete_by_mirroring",
        _ => "scan.symmetry",
    }
}

/// What Complete by Mirroring would add for the plane shown: its triangles painted, and how many.
#[derive(Clone, Debug)]
struct Filling {
    key: ([u64; 3], [u64; 3], u64),
    result: Result<(usize, u64, Arc<Vec<u8>>), String>,
}

/// Work out what OK would add when the plane or the design has changed (and OK completes).
fn fill(app: &SolveApp, k: &mut Symmetry) {
    let Some(found) = k.found.as_ref().filter(|_| k.square == COMPLETE) else { return };
    let Ok(m) = &found.mirror else { return };
    let key = (bits(Vec3::from(m.point)), bits(Vec3::from(m.normal)), app.session.revision);
    if k.filling.as_ref().is_some_and(|f| f.key == key) {
        return;
    }
    let body = found.body.as_ref().map(|b| b.0.as_str());
    let rgb = solidcraft_engine::cmd::scan::FILL_COLOR.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    let result = solidcraft_engine::cmd::scan::mirror_fill_preview(&app.session, body, Vec3::from(m.point), Vec3::from(m.normal))
        .map_err(|e| e.to_string())
        .map(|triangles| {
            let mut out = Vec::new();
            crate::scan_dots::paint_corners(&mut out, triangles.iter().map(|&corners| (corners, 0.0, rgb)));
            let version = VERSION.with(|v| {
                v.set(v.get() + 1);
                v.get()
            });
            (triangles.len(), version, Arc::new(out))
        });
    k.filling = Some(Filling { key, result });
}

/// What Complete by Mirroring would add, for the GPU viewport while the dialog shows it.
pub(crate) fn gpu_fill(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanSymmetry(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let (_, version, triangles) = k.filling.as_ref().filter(|_| k.square == COMPLETE)?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: FILL_ID,
        version: *version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: triangles.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// The scan coloured by how far each point's reflection lands from it, and what for.
#[derive(Clone, Debug)]
struct Colouring {
    key: ([u64; 3], [u64; 3], u64),
    result: Result<(f64, u64, Arc<Vec<u8>>), String>,
}

/// Search again when the guide, the plane picked or the design has changed. A plane picked (an
/// origin or construction plane) is used as it is: on a scan of one side of a part that mirrors
/// in more than one plane, no search can tell which is meant.
fn search(app: &SolveApp, k: &mut Symmetry, inputs: &[SelInput]) {
    let picked = inputs
        .first()
        .and_then(|i| i.items.iter().rev().find_map(|x| if let Sel::Plane { name } = x { Some(name.clone()) } else { None }))
        .and_then(|name| solidcraft_engine::cmd::scan::named_plane(&app.session, &name).map(|(p, n)| (name, p, n)));
    let key = (k.guide.map(|g| g.map(bits)), app.session.revision, picked.as_ref().map(|(_, p, n)| [bits(*p), bits(*n)]), k.square == COMPLETE);
    if k.found.as_ref().is_some_and(|f| f.key == key) {
        return;
    }
    let body = scan_box(&app.session, None).ok();
    let on = body.as_ref().map(|b| b.0.as_str());
    let mirror = match &picked {
        Some((_, point, normal)) => solidcraft_engine::cmd::scan::mirror_preview(&app.session, on, *point, *normal, k.square == COMPLETE),
        None => symmetry_preview(&app.session, on, k.guide, k.square == COMPLETE),
    }
    .map_err(|e| e.to_string());
    k.found = Some(Found { key, body, mirror, plane: picked.map(|(name, _, _)| name) });
}

/// Colour the scan again when the plane or the design has changed (and the colouring is on).
fn colour(app: &SolveApp, k: &mut Symmetry) {
    let Some(found) = k.found.as_ref().filter(|_| k.colour) else { return };
    let Ok(m) = &found.mirror else { return };
    let key = (bits(Vec3::from(m.point)), bits(Vec3::from(m.normal)), app.session.revision);
    if k.colouring.as_ref().is_some_and(|c| c.key == key) {
        return;
    }
    let body = found.body.as_ref().map(|b| b.0.as_str());
    let result = mismatch_preview(&app.session, body, Vec3::from(m.point), Vec3::from(m.normal)).map_err(|e| e.to_string()).and_then(|(name, mm)| {
        let shown = solidcraft_engine::cmd::scan::shown_mesh(&app.session, &name).ok_or("the scan is gone")?;
        let positions = shown.positions.as_slice();
        let [good, bad] = mismatch_colours();
        let mut dots = Vec::with_capacity(positions.len() * crate::scan_dots::DOT_SIZE);
        for (p, &v) in positions.iter().zip(&mm.values) {
            if !v.is_finite() {
                continue;
            }
            let t = (f64::from(v) / mm.limit).clamp(0.0, 1.0);
            let rgb = std::array::from_fn(|i| (f64::from(good[i]) + (f64::from(bad[i]) - f64::from(good[i])) * t).round() as u8);
            cloud_dot(&mut dots, &Dot { at: [p.x as f32, p.y as f32, p.z as f32], value: v, rgb });
        }
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Ok((mm.limit, version, Arc::new(dots)))
    });
    k.colouring = Some(Colouring { key, result });
}

/// The ends of the mismatch colouring: where a reflection lands on the scan, and a few noise widths
/// off or more (the draft analysis's good and bad colours, which follow the colour vision setting).
fn mismatch_colours() -> [[u8; 3]; 2] {
    let a = solidcraft_theme::Tokens::get().analysis;
    let byte = |c: [f32; 3]| c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    [byte(a.draft[0]), byte(a.draft[2])]
}

/// The Symmetry dialog's rows.
pub fn symmetry_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Symmetry, inputs: &[SelInput]) -> bool {
    search(app, k, inputs);
    let ids = |id: &'static str| solidcraft_ui_text::tr(id);
    if let Some(found) = &k.found {
        match &found.mirror {
            Err(why) => {
                ui_kit::text(ui, why, ui_kit::Text::Report("scan_symmetry_error", true));
            }
            Ok(m) => {
                let matched = format!("{:.0} %", 100.0 * m.coverage);
                form_row(ui, ids("ui.scan_symmetry_matched"), |ui| ui_kit::text(ui, &matched, ui_kit::Text::Cell));
                let off = crate::prefs::show_mm(app, m.rms, 1);
                form_row(ui, ids("ui.scan_symmetry_off"), |ui| ui_kit::text(ui, &off, ui_kit::Text::Cell));
                let verdict = if m.symmetric { ids("ui.scan_symmetry_yes") } else { ids("ui.scan_symmetry_no") };
                ui_kit::text(ui, verdict, ui_kit::Text::Hint("scan_symmetry_verdict"));
            }
        }
    }
    form_row(ui, ids("ui.scan_symmetry_colour"), |ui| check(ui, &mut k.colour));
    colour(app, k);
    if k.colour
        && let Some(c) = &k.colouring
    {
        match &c.result {
            Ok((limit, _, _)) => {
                let key = solidcraft_ui_text::format("ui.scan_symmetry_colour_key", &[("limit", &crate::prefs::show_mm(app, *limit, 1))]);
                ui_kit::text(ui, &key, ui_kit::Text::Hint("scan_symmetry_colour_key"));
            }
            Err(why) => {
                ui_kit::text(ui, why, ui_kit::Text::Report("scan_symmetry_colour_error", true));
            }
        }
    }
    // The Plane row above takes an origin or construction plane picked; until one is, say so.
    if k.found.as_ref().is_none_or(|f| f.plane.is_none()) {
        ui_kit::text(ui, ids("ui.scan_symmetry_plane_hint"), ui_kit::Text::Hint("scan_symmetry_plane_hint"));
    }
    form_row(ui, ids("ui.scan_symmetry_draw"), |ui| check(ui, &mut k.drawing));
    if k.guide.is_some() {
        let mut keep = true;
        form_row(ui, ids("ui.scan_symmetry_guide"), |ui| check(ui, &mut keep));
        if !keep {
            k.guide = None;
        }
    }
    let labels = [ids("ui.scan_symmetry_keep"), ids("ui.scan_symmetry_square"), ids("ui.scan_symmetry_complete")];
    choice_row(ui, ids("ui.scan_symmetry_on_ok"), "scan_symmetry_on_ok", &mut k.square, &labels);
    if k.square == COMPLETE {
        fill(app, k);
        if let Some(Ok((n, _, _))) = k.filling.as_ref().map(|f| &f.result) {
            let adds = solidcraft_ui_text::format("ui.scan_symmetry_fill_count", &[("triangles", &n.to_string())]);
            ui_kit::text(ui, &adds, ui_kit::Text::Note("scan_symmetry_fill_count"));
        }
        ui_kit::text(ui, ids("ui.scan_symmetry_complete_hint"), ui_kit::Text::Hint("scan_symmetry_complete_hint"));
        // OK adds geometry: say plainly when the scan is no mirror image about the plane.
        if let Some(m) = k.found.as_ref().and_then(|f| f.mirror.as_ref().ok()).filter(|m| !m.symmetric) {
            let share = format!("{:.0} %", 100.0 * m.coverage);
            let unlike = solidcraft_ui_text::format("ui.scan_symmetry_complete_unlike", &[("share", &share)]);
            // Expected on a scan of one side, which is what completing is for: said, not an error.
            ui_kit::text(ui, &unlike, ui_kit::Text::Report("scan_symmetry_complete_unlike", false));
        }
    }
    ui_kit::text(ui, ids("ui.scan_symmetry_hint"), ui_kit::Text::Hint("scan_symmetry_hint"));
    false
}

/// What OK runs: keep the plane shown, square the scan to it, or complete the scan by mirroring
/// it in it: the very plane shown, however much of the scan matches it.
pub fn symmetry_commands(k: &Symmetry) -> Result<Vec<(String, Value)>, String> {
    let found = k.found.as_ref().ok_or("still looking for the plane")?;
    let mirror = found.mirror.as_ref().map_err(Clone::clone)?;
    let mut params = Map::new();
    if let Some((body, _, _)) = &found.body {
        params.insert("body".into(), json!(body));
    }
    // The plane shown is the one OK uses, however much of the scan matches it.
    params.insert("plane".into(), json!({"point": mirror.point, "normal": mirror.normal}));
    Ok(vec![(symmetry_command(k).into(), Value::Object(params))])
}

/// Symmetry's colouring by mismatch for the GPU viewport while the dialog shows it.
pub(crate) fn gpu_mismatch(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanSymmetry(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let (_, version, dots) = k.colouring.as_ref().filter(|_| k.colour)?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: MISMATCH_ID,
        version: *version,
        dots: dots.clone(),
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

fn symmetry_dialog(app: &mut SolveApp) -> Option<&mut Symmetry> {
    match app.dialog.as_mut().map(|d| &mut d.kind) {
        Some(Kind::ScanSymmetry(k)) => Some(k),
        _ => None,
    }
}

/// Whether the next primary drag in the view draws a guide (it does not select or move).
pub(crate) fn drawing_guide(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSymmetry(k)) if k.drawing || k.dragging.is_some())
}

/// A guide's drag: begun at `at`, now at `to` on the screen.
pub(crate) fn guide_drag(app: &mut SolveApp, at: egui::Pos2, to: egui::Pos2) {
    if let Some(k) = symmetry_dialog(app) {
        k.dragging = Some((at, to));
    }
}

/// The drag let go: its ends are put at the depth of the scan's middle, seen the way the view
/// looks, and the plane is searched near the one through them.
pub(crate) fn guide_end(app: &mut SolveApp, proj: &crate::viewport::Proj) {
    let toward = proj.cam.back();
    let Some(k) = symmetry_dialog(app) else { return };
    let Some((a, b)) = k.dragging.take() else { return };
    k.drawing = false;
    let Some(middle) = k.found.as_ref().and_then(|f| f.body.as_ref()).map(|(_, lo, hi)| (*lo + *hi) / 2.0) else { return };
    let at_depth = |p: egui::Pos2| {
        let (o, d) = proj.ray(p);
        let den = d.dot(toward);
        (den.abs() > 1e-12).then(|| o + d * ((middle - o).dot(toward) / den))
    };
    if a.distance(b) < 4.0 {
        return;
    }
    if let (Some(from), Some(to)) = (at_depth(a), at_depth(b)) {
        k.guide = Some([from, to, toward]);
    }
}

/// The plane's sheet over the scan, and the guide while it is dragged or once drawn.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanSymmetry(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let t = solidcraft_theme::Tokens::get();
    let line = solidcraft_theme::srgb(t.analysis.draft[1]);
    let fill = line.gamma_multiply(60.0 / 255.0);
    if let Some(found) = &k.found
        && let (Ok(m), Some((_, lo, hi))) = (&found.mirror, &found.body)
    {
        let corners: Option<Vec<egui::Pos2>> =
            sheet(Vec3::from(m.point), Vec3::from(m.normal), *lo, *hi).iter().map(|p| proj.to_screen(*p)).collect();
        if let Some(corners) = corners.filter(|c| c.len() >= 3) {
            let stroke = egui::Stroke::new(1.5, line);
            if m.symmetric {
                painter.extend(crate::viewport::projected_polygon(&corners, fill, stroke));
            } else {
                painter.extend(crate::viewport::projected_polygon(&corners, fill, egui::Stroke::NONE));
                let mut closed = corners.clone();
                closed.extend(corners.first().copied());
                painter.extend(crate::ui_kit::dashed_line(&closed, proj.rect, stroke, 8.0, 6.0));
            }
        }
    }
    let guide_stroke = egui::Stroke::new(2.0, line);
    if let Some((a, b)) = k.dragging {
        painter.extend(crate::ui_kit::dashed_line(&[a, b], proj.rect, guide_stroke, 10.0, 6.0));
    } else if let Some([from, to, _]) = k.guide
        && let (Some(a), Some(b)) = (proj.to_screen(from), proj.to_screen(to))
    {
        painter.extend(crate::ui_kit::dashed_line(&[a, b], proj.rect, guide_stroke, 10.0, 6.0));
    }
}

/// Where the plane through `point` square to `normal` cuts the box `lo`..`hi`, a little larger:
/// its corners in order round it.
pub(crate) fn sheet(point: Vec3, normal: Vec3, lo: Vec3, hi: Vec3) -> Vec<Vec3> {
    let corner =
        |i: usize| Vec3::new(if i & 1 == 0 { lo.x } else { hi.x }, if i & 2 == 0 { lo.y } else { hi.y }, if i & 4 == 0 { lo.z } else { hi.z });
    let edges = [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)];
    let side = |p: Vec3| (p - point).dot(normal);
    let mut cut: Vec<Vec3> = edges
        .iter()
        .filter_map(|&(a, b)| {
            let (pa, pb) = (corner(a), corner(b));
            let (da, db) = (side(pa), side(pb));
            ((da <= 0.0) != (db <= 0.0) && (da - db).abs() > 1e-12).then(|| pa + (pb - pa) * (da / (da - db)))
        })
        .collect();
    if cut.len() < 3 {
        return Vec::new();
    }
    let middle = cut.iter().fold(Vec3::ZERO, |s, p| s + *p) / cut.len() as f64;
    let Some(u) = (cut[0] - middle).normalized() else { return Vec::new() };
    let v = normal.cross(u);
    cut.sort_by(|a, b| {
        let angle = |p: &Vec3| (*p - middle).dot(v).atan2((*p - middle).dot(u));
        angle(a).total_cmp(&angle(b))
    });
    cut.into_iter().map(|p| middle + (p - middle) * 1.08).collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solidcore_record::Vec3;

    use super::*;

    /// A slab 80 × 50 × 10 scanned lying flat, each side cut into 8 × 8 squares, as an STL.
    fn slab_stl() -> String {
        let (size, n) = ([80.0f64, 50.0, 10.0], 8u32);
        let mut stl = String::from("solid s\n");
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for high in [false, true] {
                let at = |i: u32, j: u32| {
                    let mut c = [0.0, 0.0, 0.0];
                    if high {
                        c[axis] = size[axis];
                    }
                    c[u] = size[u] * f64::from(i) / f64::from(n);
                    c[v] = size[v] * f64::from(j) / f64::from(n);
                    c
                };
                for i in 0..n {
                    for j in 0..n {
                        for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                            stl += "facet normal 0 0 0\nouter loop\n";
                            for [x, y, z] in tri {
                                stl += &format!("vertex {x} {y} {z}\n");
                            }
                            stl += "endloop\nendfacet\n";
                        }
                    }
                }
            }
        }
        stl + "endsolid s\n"
    }

    fn bounds(h: &crate::scenario::Harness) -> (Vec3, Vec3) {
        let state = h.app.session.model.state();
        let body = state.bodies.iter().find(|b| b.body.is_mesh()).unwrap();
        let (positions, _) = body.body.triangle_mesh().unwrap();
        positions
            .iter()
            .fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
    }

    /// The button opens the dialog with the steadiest base marked; the list chooses another; a face
    /// clicked on the scan is marked instead, and OK stands the slab on it, on its end.
    #[test]
    fn stand_on_base_marks_the_base_and_stands_on_the_face_clicked() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("slab.stl");
        std::fs::write(&path, slab_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.stand_on_base");
        h.frames(3);
        let marks = gpu_marks(&h.app).expect("the steadiest base is marked");
        // A broad face: 9 × 9 points of its grid.
        assert_eq!(marks.dots.len(), 81 * crate::scan_dots::DOT_SIZE);

        if let Some(Kind::ScanBase(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.index = 2;
        }
        h.frames(2);
        let other = gpu_marks(&h.app).unwrap();
        assert_ne!(other.version, marks.version, "another base, marked again");

        // The end at X = 80, clicked in the view.
        h.step(&json!({"click": {"world": [80.0, 25.0, 5.0]}})).unwrap();
        h.frames(2);
        let picked = h.app.dialog.as_ref().map(|d| d.inputs[0].items.clone()).unwrap();
        assert!(matches!(picked.first(), Some(Sel::Face { .. })), "{picked:?}");
        let end = gpu_marks(&h.app).unwrap();
        assert_eq!(end.dots.len(), 81 * crate::scan_dots::DOT_SIZE, "the end face's 9 × 9 points");

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none() && gpu_marks(&h.app).is_none());
        let (lo, hi) = bounds(&h);
        assert!(lo.z.abs() < 1e-4 && (hi.z - 80.0).abs() < 1e-4, "standing on its end: {lo:?} {hi:?}");
    }

    /// Pressed again while the slab stands where OK put it (read off the slab: OK runs on a copy of
    /// the session, which remembers nothing), the dialog opens on the next base, so
    /// OK turns it to its next way up; once it has been lifted off the grid, it opens on the
    /// steadiest again.
    #[test]
    fn stand_on_base_pressed_again_offers_the_next_base() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("slab.stl");
        std::fs::write(&path, slab_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let index = |h: &crate::scenario::Harness| match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanBase(k)) => k.index,
            _ => usize::MAX,
        };
        // Tilted, it stands on no base: the steadiest first.
        h.app.run("solid.move", json!({"bodies": ["slab"], "axis": [1, 0.3, 0], "angle": "30 deg"})).unwrap();
        h.app.start("scan.stand_on_base");
        h.frames(3);
        assert_eq!(index(&h), 0, "the steadiest first");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        let first = bounds(&h);
        h.app.start("scan.stand_on_base");
        h.frames(3);
        assert_eq!(index(&h), 1, "the next base");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert_ne!(bounds(&h), first, "turned to its next way up");
        h.app.run("solid.move", json!({"bodies": ["slab"], "translate": [0, 0, 5]})).unwrap();
        h.app.start("scan.stand_on_base");
        h.frames(3);
        assert_eq!(index(&h), 0, "moved since: the steadiest again");
    }

    /// The viewport keeps each overlay cloud by its id: two scan overlays sharing one would show
    /// each other's colours (Fit's and Clean's area marks did, and Select's and Edit Surface's).
    #[test]
    fn every_scan_overlay_has_its_own_cloud_id() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut seen = std::collections::BTreeMap::new();
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for line in text.lines().filter(|l| l.trim_start().starts_with("const ") && l.contains("u64 = u64::MAX - ")) {
                let n: u64 = line.rsplit("u64::MAX - ").next().unwrap().trim_end_matches(';').trim().parse().unwrap();
                if let Some(other) = seen.insert(n, path.display().to_string()) {
                    panic!("u64::MAX - {n} is both {other}'s and {}'s", path.display());
                }
            }
        }
        assert!(seen.len() >= 10, "{seen:?}");
    }

    /// An STL of boxes, each side cut into `n` × `n` squares.
    fn boxes_stl(boxes: &[([f64; 3], [f64; 3])], n: u32) -> String {
        boxes_stl_keeping(boxes, n, |_| true)
    }

    /// [`boxes_stl`] with only the triangles `keep` keeps: a scan that missed some of it.
    fn boxes_stl_keeping(boxes: &[([f64; 3], [f64; 3])], n: u32, keep: impl Fn(&[[f64; 3]; 3]) -> bool) -> String {
        let mut stl = String::from("solid b\n");
        for (lo, hi) in boxes {
            for axis in 0..3 {
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                for high in [false, true] {
                    let at = |i: u32, j: u32| {
                        let mut c = *lo;
                        if high {
                            c[axis] = hi[axis];
                        }
                        c[u] = lo[u] + (hi[u] - lo[u]) * f64::from(i) / f64::from(n);
                        c[v] = lo[v] + (hi[v] - lo[v]) * f64::from(j) / f64::from(n);
                        c
                    };
                    for i in 0..n {
                        for j in 0..n {
                            for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                                if !keep(&tri) {
                                    continue;
                                }
                                stl += "facet normal 0 0 0\nouter loop\n";
                                for [x, y, z] in tri {
                                    stl += &format!("vertex {x} {y} {z}\n");
                                }
                                stl += "endloop\nendfacet\n";
                            }
                        }
                    }
                }
            }
        }
        stl + "endsolid b\n"
    }

    /// Symmetry opens on the plane it finds by itself; with Draw a guide on, a line dragged across
    /// the top view along X = 31 finds the plane X = 30 near it; OK keeps it as a construction plane.
    #[test]
    fn a_guide_dragged_in_the_view_finds_the_plane_along_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("l.stl");
        std::fs::write(&path, boxes_stl(&[([0.0, 0.0, 0.0], [60.0, 20.0, 10.0]), ([20.0, 0.0, 10.0], [40.0, 20.0, 25.0])], 12)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.symmetry");
        h.frames(3);
        let normal = |h: &crate::scenario::Harness| match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanSymmetry(k)) => k.found.as_ref().and_then(|f| f.mirror.as_ref().ok()).map(|m| (Vec3::from(m.normal), m.symmetric)),
            _ => None,
        };
        let (_, symmetric) = normal(&h).expect("a plane found by itself");
        assert!(symmetric);

        // Draw a guide on (the dialog's own check box).
        if let Some(Kind::ScanSymmetry(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.drawing = true;
        }
        assert!(drawing_guide(&h.app), "the next drag draws a guide");
        h.step(&json!({"drag": [{"world": [31.0, -8.0, 12.5]}, {"world": [31.0, 28.0, 12.5]}], "steps": 6})).unwrap();
        h.frames(3);
        assert!(!drawing_guide(&h.app), "one drag, then the view is for turning again");
        let (n, symmetric) = normal(&h).unwrap();
        assert!(symmetric && n.x.abs() > 1.0 - 1e-6, "the plane X = 30: {n:?}");

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none());
        assert!(h.app.session.doc.features.iter().any(|f| f.name == "Symmetry Plane"), "kept as a construction plane");
    }

    /// Complete by Mirroring opens Symmetry with the scan coloured by mismatch, on the plane it finds
    /// by itself; the corner the scan missed (X beyond 45, Y beyond 12) lands on nothing, and OK adds
    /// it back from the other side as a body of its own.
    #[test]
    fn complete_by_mirroring_colours_the_mismatch_and_adds_what_was_missed() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("l.stl");
        let boxes = [([0.0, 0.0, 0.0], [60.0, 20.0, 10.0]), ([20.0, 0.0, 10.0], [40.0, 20.0, 25.0])];
        std::fs::write(&path, boxes_stl_keeping(&boxes, 12, |t| t.iter().any(|p| p[0] < 45.0 || p[1] < 12.0))).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.complete_by_mirroring");
        if let Some(Kind::ScanSymmetry(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            assert_eq!((k.square, k.colour), (COMPLETE, true), "on OK it completes, the mismatch shown");
        }
        h.frames(3);
        let cloud = gpu_mismatch(&h.app).expect("the scan coloured by mismatch");
        assert!(!cloud.dots.is_empty());
        let fill = gpu_fill(&h.app).expect("what OK adds, shown before it");
        assert!(!fill.triangles.is_empty());

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none() && gpu_mismatch(&h.app).is_none());
        let state = h.app.session.model.state();
        let fill = state.bodies.iter().find(|b| b.name.ends_with("Mirror Fill")).expect("the fill, a body of its own");
        let (positions, _) = fill.body.triangle_mesh().expect("a mesh");
        assert!(!positions.is_empty() && positions.iter().all(|p| p.x > 44.0 && p.y > 11.0), "only the corner the scan missed");
    }

    /// A box centred on the origin mirrors in YZ and in XZ; with its end past X = 15 never scanned,
    /// the search finds XZ (all of what is there matches it). The origin's YZ plane picked is used
    /// as it is: OK fills the missing end, mirrored in YZ.
    #[test]
    fn complete_by_mirroring_uses_the_plane_picked_as_it_is() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("box.stl");
        let boxes = [([-40.0, -20.0, 0.0], [40.0, 20.0, 30.0])];
        std::fs::write(&path, boxes_stl_keeping(&boxes, 12, |t| t.iter().all(|p| p[0] <= 15.0))).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.complete_by_mirroring");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(Sel::Plane { name: "YZ".into() });
        }
        h.frames(3);
        let commands = h.app.dialog.as_ref().and_then(|d| if let Kind::ScanSymmetry(k) = &d.kind { symmetry_commands(k).ok() } else { None });
        let (_, params) = commands.and_then(|c| c.into_iter().next()).expect("OK's command");
        let n = &params["plane"]["normal"];
        assert!(n[0].as_f64().unwrap().abs() > 1.0 - 1e-9, "the YZ plane as it is: {params}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        let state = h.app.session.model.state();
        let fill = state.bodies.iter().find(|b| b.name.ends_with("Mirror Fill")).expect("the fill");
        let (positions, _) = fill.body.triangle_mesh().expect("a mesh");
        assert!(positions.iter().any(|p| p.x > 35.0), "the missing end filled, out to X = 40");
    }

    /// A slab with a tall post on one corner is no mirror image: Complete by Mirroring says what OK
    /// would add may not be the part, before anything is added.
    #[test]
    fn complete_by_mirroring_warns_on_a_part_that_is_no_mirror_image() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("post.stl");
        let boxes = [([0.0, 0.0, 0.0], [60.0, 20.0, 10.0]), ([0.0, 0.0, 10.0], [25.0, 10.0, 45.0])];
        std::fs::write(&path, boxes_stl_keeping(&boxes, 12, |_| true)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.complete_by_mirroring");
        h.frames(3);
        let symmetric = match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanSymmetry(k)) => k.found.as_ref().and_then(|f| f.mirror.as_ref().ok()).map(|m| m.symmetric),
            _ => None,
        };
        assert_eq!(symmetric, Some(false), "the best plane found is no mirror plane");
        assert!(crate::scenario::rect_of("dialog:report:scan_symmetry_complete_unlike").is_some(), "the dialog says so");
    }
}
