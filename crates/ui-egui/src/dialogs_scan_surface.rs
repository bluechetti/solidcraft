//! Fit Surface (`scan.fit_surface`): click the scan where the surface goes; the area round the
//! click is marked on the scan and the surface fitted to it drawn over the picture as lines across
//! it, with how close it comes, as the radius, the control points and the smoothing change. OK
//! keeps it.
//!
//! Edit Surface (`scan.edit_surface`): the control net of a surface Fit Surface made is drawn over
//! the view with a handle at each point. A handle dragged moves its point in the plane of the
//! screen (keeping its depth along the view), or, with Snap to scan on, onto the scan under the
//! pointer; with Refit on, the points round it (or all but those moved by hand) are fitted to the
//! scan again as it moves, the dragged one held where it is put. The surface it would make is drawn
//! as lines across it, and how far it is from the scan is shown as it moves. OK keeps the net as
//! one undo step; Reset Points puts it back as it was.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::surface::{self as edit, Covered, EditableSurface, Line};
use solidcraft_engine::geom::bspline::NurbsSurface;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit::{self, choice_row};

/// What a drag refits (0 nothing): the ring round the point dragged, or every point not moved by
/// hand.
pub const REFIT_AROUND: usize = 1;
pub const REFIT_ALL: usize = 2;

/// How many rings round the dragged point Refit Around frees.
const RING: usize = 2;

/// A handle's half size, and how near the pointer must be pressed to take it, in pixels.
const HANDLE: f32 = 4.0;
const GRAB: f32 = 10.0;

/// The most scan points a drag's preview refits to and measures: the full set (as the command
/// uses) once it is let go.
const PREVIEW_POINTS: usize = 2_500;

/// The surface drawn as this many lines each way, each sampled at this many points.
const LINES: usize = 10;
const SAMPLES: usize = 40;

/// The Edit Surface dialog's values.
#[derive(Clone, Debug)]
pub struct EditSurface {
    /// Where a dragged point goes: free in the plane of the screen ([`SNAP_OFF`]), onto the scan
    /// under the pointer ([`SNAP_POINTER`]) or onto the scan's nearest place ([`SNAP_NEAREST`]).
    pub snap: usize,
    /// The scan is met no farther than this from where a point is dragged (mm; 0, no limit), so a
    /// point dragged over an opening stays where it is put.
    pub snap_reach: f64,
    /// The net's shape as edited (its points each way and knots: Split, Increase Resolution, an
    /// edge copied out), and whether it differs from the surface's own.
    base: Option<Box<NurbsSurface>>,
    reshaped: bool,
    /// The row or column chosen: a drag of one of its points moves it all, and the net tools act on it.
    line: Option<Line>,
    /// Why the last net tool did nothing.
    note: Option<String>,
    /// What a drag refits: 0 nothing, [`REFIT_AROUND`] or [`REFIT_ALL`].
    pub refit: usize,
    /// The surface body to edit, when it was named (opened from the history); else the selected
    /// one, or the one made last.
    target: Option<String>,
    loaded: Option<Result<Box<Loaded>, String>>,
    /// The net as edited so far, and the points moved by hand.
    net: Vec<Vec3>,
    moved: Vec<bool>,
    /// The surface shown (the net, or a drag's preview of it) and its net.
    shown: Option<NurbsSurface>,
    shown_net: Vec<Vec3>,
    /// The scan's distance from the surface shown: root mean square and largest.
    fit: Option<(f64, f64)>,
    chosen: Option<usize>,
    /// How far Move takes the chosen point along the way the surface faces, in mm.
    pub along: f64,
    drag: Option<Drag>,
}

impl Default for EditSurface {
    fn default() -> Self {
        EditSurface {
            snap: SNAP_OFF,
            snap_reach: 0.0,
            base: None,
            reshaped: false,
            line: None,
            note: None,
            refit: REFIT_AROUND,
            target: None,
            loaded: None,
            net: Vec::new(),
            moved: Vec::new(),
            shown: None,
            shown_net: Vec::new(),
            fit: None,
            chosen: None,
            along: 1.0,
            drag: None,
        }
    }
}

#[derive(Clone, Debug)]
struct Loaded {
    target: EditableSurface,
    covered: Option<Covered>,
    /// An even sample of the covered points, for previews while a handle is dragged.
    sample: Arc<Vec<Vec3>>,
    revision: u64,
}

#[derive(Clone, Debug)]
struct Drag {
    index: usize,
    /// Where the point was when the drag began: the depth it keeps.
    from: Vec3,
    /// The points that go with it (its line, when one is chosen and holds it), and where each was.
    with: Vec<(usize, Vec3)>,
}

/// Where a dragged point goes.
pub const SNAP_OFF: usize = 0;
pub const SNAP_POINTER: usize = 1;
pub const SNAP_NEAREST: usize = 2;

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    match id {
        "scan.edit_surface" => Some((Kind::ScanEditSurface(EditSurface::default()), Vec::new())),
        "scan.fit_surface" => {
            Some((Kind::ScanFitSurface(FitSurface::default()), vec![crate::selection::SelInput::new("Point", crate::selection::FACES, false)]))
        }
        _ => None,
    }
}

/// The marks' id among the clouds the viewport draws.
const FIT_CLOUD_ID: u64 = u64::MAX - 5;

thread_local! {
    static FIT_VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The Fit Surface dialog's values (the point clicked is its selection input).
#[derive(Clone, Debug)]
pub struct FitSurface {
    /// The radius of the area round the point, in mm; 0 until the scan's own default is known.
    pub radius: f64,
    pub controls: usize,
    pub smoothing: f64,
    shown: Option<FitShown>,
}

impl Default for FitSurface {
    fn default() -> Self {
        FitSurface { radius: 0.0, controls: 8, smoothing: 0.1, shown: None }
    }
}

#[derive(Clone, Debug)]
struct FitShown {
    key: ([u64; 3], u64, usize, u64, u64),
    result: Result<(solidcraft_engine::cmd::scan::SurfacePreview, u64, Arc<Vec<u8>>), String>,
}

/// The point clicked on a scan: its scan and the place.
fn clicked(inputs: &[crate::selection::SelInput]) -> Option<(String, Vec3)> {
    crate::selection::items(inputs, 0).iter().find_map(|s| match s {
        solidcraft_engine::Sel::Face { body, point, .. } => Some((body.clone(), *point)),
        _ => None,
    })
}

/// Fit again when the point, the settings or the design have changed.
fn fit(app: &SolveApp, k: &mut FitSurface, inputs: &[crate::selection::SelInput]) {
    let Some((body, point)) = clicked(inputs) else {
        k.shown = None;
        return;
    };
    if !(k.radius > 0.0) {
        k.radius = solidcraft_engine::cmd::scan::default_surface_radius(&app.session, Some(&body)).unwrap_or(10.0);
    }
    let key =
        ([point.x.to_bits(), point.y.to_bits(), point.z.to_bits()], k.radius.to_bits(), k.controls, k.smoothing.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = solidcraft_engine::cmd::scan::surface_preview(&app.session, Some(&body), point, k.radius, k.controls, k.smoothing)
        .map_err(|e| e.to_string())
        .map(|found| {
            let rgb = crate::theme::Tokens::get().analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
            let mut dots = Vec::with_capacity(found.marked.len() * solidcraft_gpu::view::DOT_SIZE);
            for at in &found.marked {
                solidcraft_gpu::view::cloud_dot(&mut dots, &solidcraft_gpu::view::Dot { at: *at, rgb, ..solidcraft_gpu::view::Dot::default() });
            }
            let version = FIT_VERSION.with(|v| {
                v.set(v.get() + 1);
                v.get()
            });
            (found, version, Arc::new(dots))
        });
    k.shown = Some(FitShown { key, result });
}

/// The Fit Surface dialog's rows (after the point input).
pub fn fit_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FitSurface, inputs: &[crate::selection::SelInput]) -> bool {
    fit(app, k, inputs);
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_fit_radius"), |ui| ui_kit::length_field(ui, &mut k.radius, &units, decimals));
    k.radius = k.radius.max(0.0);
    form_row(ui, tr("ui.scan_fit_controls"), |ui| ui_kit::number_field(ui, &mut k.controls, 4..=30, 0.2, ""));
    let format = |v: f64| format!("{v:.2}");
    form_row(ui, tr("ui.scan_fit_smoothing"), |ui| {
        ui_kit::slider(ui, &mut k.smoothing, 0.0..=1.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((found, _, _))) => {
            let note = crate::i18n::format(
                "ui.scan_fit_found",
                &[
                    ("points", &found.points.to_string()),
                    ("rms", &crate::prefs::show_mm(app, found.rms, 1)),
                    ("max", &crate::prefs::show_mm(app, found.max, 1)),
                ],
            );
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_fit_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_fit_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_fit_hint"), ui_kit::Text::Hint("scan_fit_hint"));
    false
}

/// What OK runs: the surface fitted round the point clicked.
pub fn fit_commands(k: &FitSurface, inputs: &[crate::selection::SelInput]) -> Result<Vec<(String, Value)>, String> {
    let (body, point) = clicked(inputs).ok_or_else(|| tr("ui.scan_fit_hint").to_owned())?;
    k.shown.as_ref().ok_or("still fitting")?.result.as_ref().map_err(Clone::clone)?;
    Ok(vec![(
        "scan.fit_surface".into(),
        json!({"body": body, "point": [point.x, point.y, point.z], "radius": format!("{} mm", k.radius), "controls": k.controls, "smoothing": k.smoothing}),
    )])
}

/// The area fitted to, marked on the scan while the dialog is open.
pub(crate) fn gpu_fit_marks(app: &SolveApp) -> Option<solidcraft_gpu::view::GpuCloud> {
    let Some(Kind::ScanFitSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let (_, version, dots) = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(solidcraft_gpu::view::GpuCloud {
        id: FIT_CLOUD_ID,
        version: *version,
        dots: dots.clone(),
        style: solidcraft_gpu::view::CloudStyle {
            mode: solidcraft_gpu::view::ColorMode::Own,
            range: [0.0, 1.0],
            filters: Default::default(),
            point_size: 3.0,
        },
    })
}

/// Lines across `surface`, `LINES` each way, over the picture.
pub(crate) fn draw_lines(painter: &egui::Painter, proj: &crate::viewport::Proj, surface: &NurbsSurface, stroke: egui::Stroke) {
    let [[u0, u1], [v0, v1]] = surface.domain();
    let at = |i: usize, n: usize, a: f64, b: f64| a + (b - a) * i as f64 / n as f64;
    for along_u in [true, false] {
        for i in 0..=LINES {
            let curve: Option<Vec<egui::Pos2>> = (0..=SAMPLES)
                .map(|j| {
                    let (u, v) =
                        if along_u { (at(j, SAMPLES, u0, u1), at(i, LINES, v0, v1)) } else { (at(i, LINES, u0, u1), at(j, SAMPLES, v0, v1)) };
                    surface.eval(u, v).ok().and_then(|p| proj.to_screen(p))
                })
                .collect();
            if let Some(curve) = curve {
                painter.add(egui::Shape::line(curve, stroke));
            }
        }
    }
}

/// Load the surface and the scan under it when the dialog opens, or again when the surface has
/// changed under it (an undo, say); what was moved is dropped then. A change elsewhere (a click
/// that selects, another body) keeps the edits.
fn load(app: &SolveApp, k: &mut EditSurface) {
    let revision = app.session.revision;
    match &mut k.loaded {
        Some(Ok(l)) if l.revision == revision => return,
        Some(Err(_)) => return,
        _ => {}
    }
    let cmd = "scan.edit_surface";
    let found = edit::editable(&app.session, k.target.as_deref(), cmd).map_err(|e| e.to_string());
    if let (Some(Ok(l)), Ok(target)) = (&mut k.loaded, &found)
        && l.target.feature == target.feature
        && l.target.surface == target.surface
    {
        l.revision = revision;
        return;
    }
    let loaded = found.map(|target| {
        let reach = edit::default_reach(&target.surface);
        let covered = edit::covered(&app.session, None, &target.surface, reach, cmd).ok().filter(|c| !c.points.is_empty());
        let all = covered.as_ref().map(|c| c.points.as_slice()).unwrap_or_default();
        let sample = Arc::new(all.iter().step_by(all.len() / PREVIEW_POINTS + 1).copied().collect());
        Box::new(Loaded { target, covered, sample, revision })
    });
    let surface = loaded.as_ref().ok().map(|l| l.target.surface.clone());
    k.loaded = Some(loaded);
    if let Some(surface) = surface {
        reset_to(k, surface);
    }
}

/// The dialog started again from `surface`: nothing moved, nothing chosen.
fn reset_to(k: &mut EditSurface, surface: NurbsSurface) {
    k.net = edit::control_points(&surface);
    k.moved = vec![false; k.net.len()];
    k.chosen = None;
    k.line = None;
    k.drag = None;
    k.reshaped = false;
    k.base = Some(Box::new(surface.clone()));
    show(k, surface, false);
}

/// The net's shape as edited.
fn base(k: &EditSurface) -> Option<&NurbsSurface> {
    k.base.as_deref().or_else(|| loaded(k).map(|l| &l.target.surface))
}

/// The net changed as a whole by `change` (a net tool), what was moved kept moved where it still is;
/// `then` the line to choose after.
fn reshape(k: &mut EditSurface, change: impl FnOnce(&NurbsSurface) -> Result<(NurbsSurface, Option<Line>), edit::NetError>) -> Result<(), String> {
    let current = k.shown.clone().or_else(|| base(k).cloned()).ok_or("no surface")?;
    let (made, then) = change(&current).map_err(|e| e.to_string())?;
    let moved_at: Vec<Vec3> = k.net.iter().zip(&k.moved).filter(|(_, m)| **m).map(|(p, _)| *p).collect();
    k.net = edit::control_points(&made);
    k.moved = k.net.iter().map(|p| moved_at.iter().any(|q| q.dist(*p) < 1e-9)).collect();
    k.base = Some(Box::new(made.clone()));
    k.reshaped = true;
    k.line = then;
    k.chosen = then.and_then(|l| l.points(made.count()).first().copied());
    show(k, made, false);
    Ok(())
}

fn loaded(k: &EditSurface) -> Option<&Loaded> {
    k.loaded.as_ref()?.as_deref().ok()
}

/// Show `surface`, with how far the scan is from it (measured on the sample while `dragging`).
fn show(k: &mut EditSurface, surface: NurbsSurface, dragging: bool) {
    k.fit = loaded(k).and_then(|l| {
        let c = l.covered.as_ref()?;
        Some(edit::distance(&surface, if dragging { &l.sample } else { &c.points }))
    });
    k.shown_net = edit::control_points(&surface);
    k.shown = Some(surface);
}

/// The surface with the points `moves` put where they go, and, as Refit says, the points round
/// them or all those not moved by hand fitted to the scan again with them held.
fn preview(k: &EditSurface, moves: &[(usize, Vec3)]) -> Option<NurbsSurface> {
    let l = loaded(k)?;
    let mut net = k.net.clone();
    let mut moved = k.moved.clone();
    for &(index, to) in moves {
        *net.get_mut(index)? = to;
        moved[index] = true;
    }
    let surface = edit::with_points(base(k)?, &net)?;
    let points = l.sample.as_slice();
    let count = surface.count();
    let held: Vec<bool> = match k.refit {
        REFIT_AROUND => {
            let mut near = vec![false; net.len()];
            for &(index, _) in moves {
                for (n, r) in edit::around(count, index % count[0], index / count[0], RING).into_iter().enumerate() {
                    near[n] |= r;
                }
            }
            near.iter().zip(&moved).map(|(near, m)| !near || *m).collect()
        }
        REFIT_ALL => moved,
        _ => return Some(surface),
    };
    Some(edit::refit(&surface, points, &held).unwrap_or(surface))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface) -> bool {
    load(app, k);
    let l = match &k.loaded {
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_edit_error", true));
            return false;
        }
        Some(Ok(l)) => l.clone(),
        None => return false,
    };
    let [nu, nv] = base(k).map_or(l.target.surface.count(), NurbsSurface::count);
    let name = format!("{} ({nu} × {nv})", l.target.body);
    form_row(ui, crate::i18n::tr_label("Surface"), |ui| ui_kit::text(ui, &name, ui_kit::Text::Cell));
    if l.covered.is_some() {
        let snaps = [tr("ui.scan_edit_snap_off"), tr("ui.scan_edit_snap_pointer"), tr("ui.scan_edit_snap_nearest")];
        choice_row(ui, tr("ui.scan_edit_snap"), "scan_edit_snap", &mut k.snap, &snaps);
        if k.snap != SNAP_OFF {
            let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
            form_row(ui, tr("ui.scan_edit_snap_reach"), |ui| ui_kit::length_field(ui, &mut k.snap_reach, &units, decimals));
            k.snap_reach = k.snap_reach.max(0.0);
        }
        let labels = [tr("ui.scan_edit_refit_off"), tr("ui.scan_edit_refit_around"), tr("ui.scan_edit_refit_all")];
        choice_row(ui, tr("ui.scan_edit_refit"), "scan_edit_refit", &mut k.refit, &labels);
    } else {
        ui_kit::text(ui, tr("ui.scan_edit_no_scan"), ui_kit::Text::Hint("scan_edit_no_scan"));
    }
    let moved = k.moved.iter().filter(|m| **m).count().to_string();
    form_row(ui, tr("ui.scan_edit_moved"), |ui| ui_kit::text(ui, &moved, ui_kit::Text::Cell));
    if let Some((rms, max)) = k.fit {
        let fit = crate::i18n::format(
            "ui.scan_edit_distance_value",
            &[("rms", &crate::prefs::show_mm(app, rms, 1)), ("max", &crate::prefs::show_mm(app, max, 1))],
        );
        form_row(ui, tr("ui.scan_edit_distance"), |ui| ui_kit::text(ui, &fit, ui_kit::Text::Cell));
    }
    let edited = k.moved.iter().any(|m| *m) || k.reshaped;
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_reset"), ui_kit::Button::Enabled(edited))).clicked() {
        reset_to(k, l.target.surface.clone());
    }
    net_tools(ui, k, [nu, nv]);
    // The point chosen (clicked or last dragged): where it goes, typed, or a distance along the
    // way the surface faces.
    if let Some(i) = k.chosen.filter(|i| *i < k.net.len()) {
        let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
        let which = format!("[{}, {}]", i % nu, i / nu);
        form_row(ui, tr("ui.scan_edit_point"), |ui| ui_kit::text(ui, &which, ui_kit::Text::Cell));
        let p = k.net[i];
        let mut xyz = [p.x, p.y, p.z];
        let mut typed = false;
        for (axis, v) in ["X", "Y", "Z"].into_iter().zip(&mut xyz) {
            form_row(ui, axis, |ui| {
                let r = ui_kit::length_field(ui, v, &units, decimals);
                typed |= r.changed();
                r
            });
        }
        if typed {
            move_to(k, i, Vec3::new(xyz[0], xyz[1], xyz[2]));
        }
        form_row(ui, tr("ui.scan_edit_along"), |ui| ui_kit::length_field(ui, &mut k.along, &units, decimals));
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_move"), ui_kit::Button::Normal)).clicked()
            && let Some(n) = k.shown.as_ref().and_then(|s| edit::normal_at(s, i))
        {
            let to = k.net[i] + n * k.along;
            move_to(k, i, to);
        }
    }
    ui_kit::text(ui, tr("ui.scan_edit_hint"), ui_kit::Text::Hint("scan_edit_hint"));
    false
}

/// The net tools (SurfaceCraft's, after QuickSurface's), on the line chosen: choose the row or
/// column of the point chosen; split it, copy it beside itself, take it away, crease it or make it
/// smooth; and double the net's points. Each changes the dialog's net; OK keeps it.
fn net_tools(ui: &mut egui::Ui, k: &mut EditSurface, count: [usize; 2]) {
    let mut outcome: Option<Result<(), String>> = None;
    if let Some(i) = k.chosen.filter(|i| *i < count[0] * count[1]) {
        let (row, column) = (Line::Row(i / count[0]), Line::Column(i % count[0]));
        ui_kit::row(ui, tr("ui.scan_edit_choose"), |ui| {
            ui.horizontal(|ui| {
                if ui_kit::button(ui, tr("ui.scan_edit_row"), if k.line == Some(row) { ui_kit::Button::SmallSelected } else { ui_kit::Button::Small })
                    .clicked()
                {
                    k.line = if k.line == Some(row) { None } else { Some(row) };
                }
                if ui_kit::button(
                    ui,
                    tr("ui.scan_edit_column"),
                    if k.line == Some(column) { ui_kit::Button::SmallSelected } else { ui_kit::Button::Small },
                )
                .clicked()
                {
                    k.line = if k.line == Some(column) { None } else { Some(column) };
                }
            })
            .response
        });
    }
    if let Some(line) = k.line {
        let creased = base(k).is_some_and(|b| solidcraft_engine::cmd::scan::surface::creases(b).contains(&line));
        ui_kit::row(ui, tr("ui.scan_edit_line"), |ui| {
            ui.horizontal(|ui| {
                if ui_kit::button(ui, tr("ui.scan_edit_split"), ui_kit::Button::Small).clicked() {
                    outcome = Some(reshape(k, |s| edit::split_line(s, line).map(|(m, l)| (m, Some(l)))));
                }
                if ui_kit::button(ui, tr("ui.scan_edit_copy"), ui_kit::Button::Small).clicked() {
                    outcome = Some(reshape(k, |s| edit::duplicate_line(s, line, true).map(|(m, l)| (m, Some(l)))));
                }
                if ui_kit::button(ui, tr("ui.scan_edit_remove"), ui_kit::Button::Small).clicked() {
                    outcome = Some(reshape(k, |s| edit::remove_line(s, line).map(|m| (m, None))));
                }
                let crease = if creased { tr("ui.scan_edit_smooth") } else { tr("ui.scan_edit_crease") };
                if ui_kit::button(ui, crease, ui_kit::Button::Small).clicked() {
                    outcome = Some(reshape(k, |s| {
                        if creased { edit::uncrease_line(s, line) } else { edit::crease_line(s, line) }.map(|m| (m, Some(line)))
                    }));
                }
            })
            .response
        });
    }
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_refine"), ui_kit::Button::Normal)).clicked() {
        outcome = Some(reshape(k, |s| edit::refine_net(s).map(|m| (m, None))));
    }
    if let Some(Err(why)) = outcome {
        k.note = Some(why);
    } else if outcome.is_some() {
        k.note = None;
    }
    if let Some(note) = &k.note {
        ui_kit::text(ui, note, ui_kit::Text::Report("scan_edit_note", true));
    }
}

/// Point `index` put at `to`, as a drag let go there would: refitted as Refit says, and kept.
fn move_to(k: &mut EditSurface, index: usize, to: Vec3) {
    if let Some(surface) = preview(k, &[(index, to)]) {
        show(k, surface, false);
        k.net = k.shown_net.clone();
        if let Some(m) = k.moved.get_mut(index) {
            *m = true;
        }
    }
}

/// What OK runs: the net as edited.
pub fn commands(k: &EditSurface) -> Result<Vec<(String, Value)>, String> {
    let l = k.loaded.as_ref().ok_or("still loading the surface")?.as_ref().map_err(Clone::clone)?;
    if !k.moved.iter().any(|m| *m) && !k.reshaped {
        return Err(crate::i18n::tr("ui.scan_edit_nothing").into());
    }
    let points: Vec<[f64; 3]> = k.net.iter().map(|p| [p.x, p.y, p.z]).collect();
    match base(k).filter(|_| k.reshaped) {
        // The net's shape changed: the whole of it.
        Some(b) => {
            let net = json!({"count": b.count(), "knots": [b.knots()[0].clone(), b.knots()[1].clone()], "degree": b.degree(), "points": points});
            Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "net": net}))])
        }
        None => Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "points": points}))]),
    }
}

fn dialog(app: &mut SolveApp) -> Option<&mut EditSurface> {
    match app.dialog.as_mut().map(|d| &mut d.kind) {
        Some(Kind::ScanEditSurface(k)) => Some(k),
        _ => None,
    }
}

/// A surface that Fit Surface, New Free Form or Wrap Surface made, edited from the history
/// (double-clicked, or Edit Feature): Edit Surface opens on it where it is. Its edits are written
/// into that feature, so the design is not rolled back to before it as other features are. False
/// when the feature is not such a surface (or is rolled back), to be edited the usual way.
pub(crate) fn edit_from_history(app: &mut SolveApp, feature: u64) -> bool {
    let Some(f) = app.session.doc.feature(feature) else { return false };
    if !matches!(f.kind, solidcraft_engine::doc::FeatureKind::Import { .. }) {
        return false;
    }
    let mut names = f.body_names.clone();
    names.extend(app.session.model.state().bodies.iter().filter(|b| b.feature == feature).map(|b| b.name.clone()));
    let Some(target) = names.iter().find_map(|n| edit::editable(&app.session, Some(n), "scan.edit_surface").ok().filter(|t| t.feature == feature))
    else {
        return false;
    };
    // Selected as well, so a dialog that opens a frame later (while the history computes) takes it too.
    let _ = app.run("select.set", json!({ "items": [{ "type": "body", "name": target.body }] }));
    app.start("scan.edit_surface");
    if let Some(k) = dialog(app) {
        k.target = Some(target.body);
        k.loaded = None;
    }
    true
}

/// Whether the dialog shows handles (the next primary press on one takes it).
pub(crate) fn editing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if loaded(k).is_some())
}

/// Whether a handle is being dragged: the drag neither selects nor turns the view.
pub(crate) fn holds_pointer(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if k.drag.is_some())
}

/// The handle under `at` on the screen, if any.
fn handle_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<usize> {
    k.shown_net
        .iter()
        .enumerate()
        .filter_map(|(i, p)| proj.to_screen(*p).map(|s| (i, s.distance(at))))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// A click at `at`: chooses the handle under it, to type where its point goes.
pub(crate) fn choose(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    if let Some(k) = dialog(app)
        && let Some(index) = handle_at(k, proj, at)
    {
        k.chosen = Some(index);
    }
}

/// A primary press at `at`: takes the handle under it, if any. With `copy` (Alt held) a point on
/// an edge of the net has its edge copied beside itself first and the copy taken, to drag the
/// surface out (QuickSurface's Alt-drag).
pub(crate) fn grab(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, copy: bool) -> bool {
    let Some(k) = dialog(app) else { return false };
    let Some(mut index) = handle_at(k, proj, at) else { return false };
    // A drag starts from the net as shown (a refit the last drag made is kept).
    k.net = k.shown_net.clone();
    if copy && let Some(count) = base(k).map(NurbsSurface::count) {
        let (i, j) = (index % count[0], index / count[0]);
        let (row_edge, column_edge) = (j == 0 || j + 1 == count[1], i == 0 || i + 1 == count[0]);
        let edge = match (k.line, row_edge, column_edge) {
            (Some(l), _, _) if l.points(count).contains(&index) && matches!(l, Line::Row(r) if r == 0 || r + 1 == count[1]) => Some(l),
            (Some(l), _, _) if l.points(count).contains(&index) && matches!(l, Line::Column(c) if c == 0 || c + 1 == count[0]) => Some(l),
            (_, true, false) => Some(Line::Row(j)),
            (_, false, true) => Some(Line::Column(i)),
            _ => None,
        };
        if let Some(edge) = edge {
            let after = match edge {
                Line::Row(r) => r + 1 == count[1],
                Line::Column(c) => c + 1 == count[0],
            };
            if reshape(k, |s| edit::duplicate_line(s, edge, after).map(|(m, l)| (m, Some(l)))).is_ok()
                && let (Some(line), Some(count)) = (k.line, base(k).map(NurbsSurface::count))
            {
                // The same place along the copy.
                let along = match edge {
                    Line::Row(_) => i,
                    Line::Column(_) => j,
                };
                index = line.points(count).get(along).copied().unwrap_or(index);
            }
        }
    }
    let count = base(k).map(NurbsSurface::count);
    let with: Vec<(usize, Vec3)> = match (k.line, count) {
        (Some(l), Some(c)) if l.points(c).contains(&index) => l.points(c).into_iter().filter_map(|p| k.net.get(p).map(|q| (p, *q))).collect(),
        _ => vec![(index, k.net[index])],
    };
    k.drag = Some(Drag { index, from: k.net[index], with });
    k.chosen = Some(index);
    true
}

/// The handle dragged to `to` on the screen.
pub(crate) fn drag_to(app: &mut SolveApp, proj: &crate::viewport::Proj, to: egui::Pos2) {
    let Some((drag, snap, reach, scan)) =
        dialog(app).and_then(|k| Some((k.drag.clone()?, k.snap, k.snap_reach, loaded(k)?.covered.as_ref().map(|c| c.scan.clone()))))
    else {
        return;
    };
    let reach = if reach > 0.0 { reach } else { f64::INFINITY };
    // In the plane of the screen through where the point was.
    let in_plane = {
        let (o, d) = proj.ray(to);
        let n = proj.cam.back();
        let den = d.dot(n);
        (den.abs() > 1e-12).then(|| o + d * ((drag.from - o).dot(n) / den))
    };
    // Onto the scan under the pointer, when snapping so and there is scan there within reach.
    let under = if snap == SNAP_POINTER
        && let Some(scan) = &scan
    {
        crate::viewport::pick_cached(app, proj, to).into_iter().find_map(|h| match h {
            crate::viewport::Hit::Face { body, point, .. } if body == *scan => Some(point),
            _ => None,
        })
    } else {
        None
    };
    let Some(place) = under.filter(|p| in_plane.is_none_or(|q| p.dist(q) <= reach)).or(in_plane) else { return };
    let delta = place - drag.from;
    // Each point that goes moved as the one dragged; snapping to the nearest place puts each on the
    // scan's nearest place to where it would go, within reach.
    let nearest = (snap == SNAP_NEAREST)
        .then(|| scan.as_deref().and_then(|b| solidcraft_engine::cmd::scan::curve::projector(&app.session, Some(b), "scan.edit_surface").ok()));
    let moves: Vec<(usize, Vec3)> = drag
        .with
        .iter()
        .map(|&(i, from)| {
            let to = if i == drag.index && under.is_some() { place } else { from + delta };
            let to = match nearest.as_ref().and_then(|n| n.as_ref()) {
                Some((_, on)) => on.nearest(to).filter(|(_, d)| *d <= reach).map_or(to, |(q, _)| q),
                None => to,
            };
            (i, to)
        })
        .collect();
    let Some(k) = dialog(app) else { return };
    if let Some(surface) = preview(k, &moves) {
        show(k, surface, true);
    }
}

/// The drag let go: the net as previewed is kept and the point counts as moved by hand.
pub(crate) fn release(app: &mut SolveApp) {
    let Some(k) = dialog(app) else { return };
    let Some(drag) = k.drag.take() else { return };
    if k.shown_net.len() == k.net.len() && k.shown_net.get(drag.index) != k.net.get(drag.index) {
        k.net = k.shown_net.clone();
        for (i, _) in &drag.with {
            if let Some(m) = k.moved.get_mut(*i) {
                *m = true;
            }
        }
    }
    // Measured again on every covered point, as the command will.
    if let Some(surface) = k.shown.clone() {
        show(k, surface, false);
    }
}

/// The surface shown, its control net and the handles, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let t = crate::theme::Tokens::get();
    if let Some(Kind::ScanFitSurface(k)) = app.dialog.as_ref().map(|d| &d.kind)
        && let Some(Ok((found, _, _))) = k.shown.as_ref().map(|s| &s.result)
    {
        draw_lines(painter, proj, &found.surface, egui::Stroke::new(1.5, t.surface_accent));
        return;
    }
    let Some(Kind::ScanEditSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(surface) = &k.shown else { return };
    draw_lines(painter, proj, surface, egui::Stroke::new(1.5, t.surface_accent));
    let [nu, nv] = surface.count();
    let screen: Vec<Option<egui::Pos2>> = k.shown_net.iter().map(|p| proj.to_screen(*p)).collect();
    let net = egui::Stroke::new(1.0, t.text.gamma_multiply(0.45));
    for j in 0..nv {
        for i in 0..nu {
            let here = screen.get(j * nu + i).copied().flatten();
            for next in [(i + 1 < nu).then(|| j * nu + i + 1), (j + 1 < nv).then(|| (j + 1) * nu + i)].into_iter().flatten() {
                if let (Some(a), Some(b)) = (here, screen.get(next).copied().flatten()) {
                    painter.line_segment([a, b], net);
                }
            }
        }
    }
    let [r, g, b] = t.analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    let moved_fill = egui::Color32::from_rgb(r, g, b);
    for (i, s) in screen.iter().enumerate() {
        let Some(s) = s else { continue };
        let on_line = k.line.zip(k.shown.as_ref().map(NurbsSurface::count)).is_some_and(|(l, c)| l.points(c).contains(&i));
        let fill = if k.chosen == Some(i) {
            t.accent
        } else if on_line {
            t.accent_soft
        } else if k.moved.get(i).copied().unwrap_or(false) {
            moved_fill
        } else {
            t.text
        };
        let rect = egui::Rect::from_center_size(*s, egui::vec2(2.0 * HANDLE, 2.0 * HANDLE));
        painter.rect_filled(rect, 1.0, fill);
        painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0, t.panel), egui::StrokeKind::Outside);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A dome scanned as a grid, `n` squares a side over 60 mm, as an STL.
    fn dome_stl(n: u32) -> String {
        let z = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        let at = |i: u32, j: u32| {
            let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
            [x, y, z(x, y)]
        };
        let mut stl = String::from("solid d\n");
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
        stl + "endsolid d\n"
    }

    fn kind(h: &crate::scenario::Harness) -> &EditSurface {
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanEditSurface(k)) => k,
            _ => panic!("the Edit Surface dialog"),
        }
    }

    /// Editing a fitted surface from the history (a double-click) opens Edit Surface on that one,
    /// not on the one made last, and leaves the design where it is; other features edit as ever.
    #[test]
    fn a_surface_edited_from_the_history_opens_edit_surface_on_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [20, 20, 7], "radius": 12, "controls": 5})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [40, 40, 7], "radius": 12, "controls": 5})).unwrap();
        h.app.run("select.clear", json!({})).unwrap();
        let first = h.app.session.doc.features.iter().find(|f| f.name == "Fit Surface").map(|f| f.id).unwrap();
        let marker = h.app.session.doc.marker;
        h.app.edit_feature(first);
        h.frames(3);
        let target = loaded(kind(&h)).map(|l| l.target.clone()).expect("the surface loaded");
        assert_eq!((target.body.as_str(), target.feature), ("Surface1", first), "the surface double-clicked, not the one made last");
        assert_eq!(h.app.session.doc.marker, marker, "nothing rolled back");
        assert!(editing(&h.app), "its handles shown");
        // The scan's own feature is not a surface: it does not open Edit Surface.
        let scan = h.app.session.doc.features.iter().find(|f| matches!(f.kind, solidcraft_engine::doc::FeatureKind::MeshImport { .. })).unwrap().id;
        h.app.edit_feature(scan);
        h.frames(2);
        assert!(!matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(_))));
    }

    /// A handle of a fitted surface dragged up through the viewport's own input moves its point and
    /// refits the ring round it; OK keeps the edit as one undo step, which puts it back.
    #[test]
    fn a_handle_dragged_in_the_view_moves_its_point_and_ok_keeps_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [30, 30, 8], "radius": 25, "controls": 6})).unwrap();
        h.step(&json!({"view_anim": "front"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert!(editing(&h.app), "handles shown");
        let start = kind(&h).shown_net.clone();
        let fit = kind(&h).fit.expect("the scan's distance from the surface");
        assert!(fit.0 < 0.15, "fitted closely to start with: {fit:?}");

        // The point at [2, 2] dragged 6 mm up in the front view.
        let k = 2 * 6 + 2;
        let (from, to) = (start[k], start[k] + Vec3::new(0.0, 0.0, 6.0));
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 6})).unwrap();
        h.frames(2);
        let after = kind(&h).clone();
        assert!(after.drag.is_none() && after.moved[k], "moved by hand");
        assert!(after.net[k].z - from.z > 4.0, "followed the pointer up: {:?} → {:?}", from, after.net[k]);
        let ring = (0..36).filter(|&i| i != k && after.net[i].dist(start[i]) > 1e-6).count();
        assert!(ring > 0, "the ring round it refitted as it moved");
        assert!(after.fit.is_some_and(|f| f.0 > fit.0), "farther from the scan now");

        // Clicked, a handle is chosen; Move takes its point 1 mm along the way the surface faces.
        let j = 3 * 6 + 3;
        let other = h.app.dialog.as_ref().map(|_| kind(&h).net[j]).unwrap();
        h.step(&json!({"click": {"world": [other.x, other.y, other.z]}})).unwrap();
        h.frames(2);
        assert_eq!(kind(&h).chosen, Some(j), "chosen by a click");
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.refit = 0;
            let n = k.shown.as_ref().and_then(|s| edit::normal_at(s, j)).unwrap();
            move_to(k, j, k.net[j] + n * k.along);
            assert!(k.moved[j] && (k.net[j] - other).dot(n) > 0.999, "1 mm along the normal");
        }
        let edited = kind(&h).net.clone();

        h.frames(2);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let kept = edit::editable(&h.app.session, Some("Surface1"), "test").unwrap();
        let net = edit::control_points(&kept.surface);
        assert!(net.iter().zip(&edited).all(|(a, b)| a.dist(*b) < 1e-6), "the net as edited, kept");
        h.app.run("edit.undo", json!({})).unwrap();
        let undone = edit::editable(&h.app.session, Some("Surface1"), "test").unwrap();
        assert!(edit::control_points(&undone.surface)[k].dist(from) < 1e-6, "one undo puts it back");
    }

    /// Fit Surface opens a dialog: a click on the dome marks the area round it and fits the
    /// surface, a smaller radius fits a smaller area, and OK keeps it.
    #[test]
    fn fit_surface_fits_round_the_point_clicked() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.fit_surface");
        h.frames(2);
        let top = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        h.step(&json!({"click": {"world": [30.0, 30.0, top(30.0, 30.0)]}})).unwrap();
        h.frames(2);
        let marks = gpu_fit_marks(&h.app).expect("the area marked");
        let count = |h: &crate::scenario::Harness| match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanFitSurface(k)) => k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).map(|(f, _, _)| (f.points, f.rms)),
            _ => None,
        };
        let (wide, rms) = count(&h).expect("fitted");
        assert!(rms < 0.05, "close to the dome: {rms}");
        if let Some(Kind::ScanFitSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            assert!(k.radius > 0.0, "the scan's own radius to start with");
            k.radius /= 2.0;
            k.controls = 4;
        }
        h.frames(2);
        let (narrow, _) = count(&h).unwrap();
        assert!(narrow < wide, "a smaller radius, a smaller area: {narrow} < {wide}");
        assert_ne!(gpu_fit_marks(&h.app).unwrap().version, marks.version, "marked again");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.model.state().body("Surface1").is_some(), "kept");
    }
}
