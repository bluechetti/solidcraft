//! The scan-cleaning dialogs. Remove Floating Pieces (`scan.remove_floating`): the scan's pieces are
//! found once, and the slider says how small a piece must be, against the largest, to float; the
//! pieces that would go are painted on the scan (in the analysis colours' bad colour) and counted.
//! A click on a piece turns it over: one that would go stays, one that would stay goes. OK takes
//! them off, one undo step.
//!
//! Fill Holes (`scan.fill_holes`): the scan's holes and rims are found once and outlined over the
//! picture, those it would close in the accent colour and the rest dimmed; the size limit ticks
//! the holes within it, and a click by an outline ticks or unticks that ring (a rim ticked is
//! capped). OK closes the ticked ones, one undo step.
//!
//! Reduce Mesh (`scan.reduce`): how much of the scan to keep (and the count it means), the most a
//! collapse may move the surface, and whether the rim stays put; OK reduces it, one undo step.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::cmd::scan::clean;
use solidcraft_engine::doc::scan_clean::CleanStep;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use solidcore_record::Vec3;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit;

/// The marks' id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 4;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Remove Floating Pieces dialog's values.
#[derive(Clone, Debug)]
pub struct Floating {
    /// The share of the largest piece below which a piece floats, as log₁₀ of a percentage.
    pub log_percent: f64,
    shown: Option<Shown>,
}

impl Default for Floating {
    fn default() -> Self {
        Floating { log_percent: (100.0 * clean::FLOATING_SHARE).log10(), shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (u64, u64, String),
    result: Result<Marked, String>,
}

#[derive(Clone, Debug)]
struct Marked {
    body: String,
    /// The clicks that keep a piece the size would take off, and that take off one it would keep.
    keep: Vec<[f64; 3]>,
    remove: Vec<[f64; 3]>,
    pieces: usize,
    floating: usize,
    triangles: usize,
    version: u64,
    dots: Arc<Vec<u8>>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    match id {
        "scan.remove_floating" => {
            Some((Kind::ScanFloating(Floating::default()), vec![crate::selection::SelInput::new("Point", crate::selection::FACES, true).places()]))
        }
        "scan.fill_holes" => Some((Kind::ScanHoles(Holes::default()), Vec::new())),
        "scan.reduce" => Some((Kind::ScanReduce(Reduce::default()), Vec::new())),
        "scan.clean_up" => Some((Kind::ScanCleanUp(CleanUp::default()), Vec::new())),
        _ => None,
    }
}

/// A tool's dialog `kind` set to the settings of the Scan Clean `step` being edited.
pub fn refill(kind: &mut Kind, step: &CleanStep) {
    match (kind, step) {
        (Kind::ScanFloating(k), CleanStep::RemoveFloating { below, .. }) => k.log_percent = (100.0 * below).max(MIN_PERCENT).log10(),
        (Kind::ScanHoles(k), CleanStep::FillHoles { max_edges, fair, at }) => {
            k.max_edges = *max_edges as usize;
            k.fair = *fair;
            k.at = at.iter().map(|p| solidcore_record::Vec3::new(p[0], p[1], p[2])).collect();
            k.by_hand = !at.is_empty();
        }
        (Kind::ScanReduce(k), CleanStep::Reduce { ratio, max_error, keep_rim, .. }) => {
            k.percent = ratio.map_or(50.0, |r| (100.0 * r).clamp(1.0, 99.0));
            k.max_error = *max_error;
            k.keep_rim = *keep_rim;
        }
        (Kind::ScanDenoise(k), CleanStep::Denoise { strength: Some(strength) }) => *k = crate::dialogs_scan_denoise::Denoise::chosen(*strength),
        _ => {}
    }
}

/// The smallest share the Remove Floating slider shows, in percent.
const MIN_PERCENT: f64 = 0.01;

impl Floating {
    /// The share of the largest piece, 0 to 0.5.
    pub fn share(&self) -> f64 {
        (10f64.powf(self.log_percent) / 100.0).clamp(0.0, 0.5)
    }
}

/// Mark the floating pieces again when the share or the design has changed.
fn mark(app: &SolveApp, k: &mut Floating, inputs: &[crate::selection::SelInput]) {
    let clicks: Vec<(String, solidcore_record::Vec3)> = crate::selection::items(inputs, 0)
        .iter()
        .filter_map(|s| match s {
            solidcraft_engine::Sel::Face { body, point, .. } => Some((body.clone(), *point)),
            _ => None,
        })
        .collect();
    let key = (k.log_percent.to_bits(), app.session.revision, format!("{clicks:?}"));
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let share = k.share();
    let result = clean::pieces_of(&app.session, None, "scan.remove_floating").map_err(|e| e.to_string()).and_then(|(body, found)| {
        let mut floating = found.floating(share);
        // Each click turns its piece over (twice, back); a point of the piece's own finds it again
        // as the step runs.
        let mut flips: Vec<Option<solidcore_record::Vec3>> = vec![None; floating.len()];
        for (_, at) in clicks.iter().filter(|(b, _)| *b == body) {
            if let Some(f) = clean::piece_at(&app.session, &body, &found, *at).and_then(|p| flips.get_mut(p)) {
                *f = if f.is_some() { None } else { Some(*at) };
            }
        }
        let (mut keep, mut remove) = (Vec::new(), Vec::new());
        for (f, flip) in floating.iter_mut().zip(&flips) {
            if let Some(a) = flip {
                *f = !*f;
                if *f { &mut remove } else { &mut keep }.push([a.x, a.y, a.z]);
            }
        }
        let state = app.session.model.state();
        let (positions, triangles) = state.body(&body).and_then(|b| b.body.triangle_mesh()).ok_or("the scan is gone")?;
        let rgb = solidcraft_theme::Tokens::get().analysis.draft[2].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut dots = Vec::new();
        let going = |t: usize| found.of.get(t).is_some_and(|&p| p != u32::MAX && floating.get(p as usize) == Some(&true));
        let count = (0..triangles.len()).filter(|&t| going(t)).count();
        // Every so many past the most a colouring paints.
        let step = count.div_ceil(crate::scan_dots::PAINT_MOST).max(1);
        let at = |i: u32| positions.get(i as usize).copied();
        for tri in triangles.iter().enumerate().filter(|(t, _)| going(*t)).step_by(step).map(|(_, tri)| tri) {
            if let (Some(a), Some(b), Some(c)) = (at(tri[0]), at(tri[1]), at(tri[2])) {
                crate::scan_dots::cloud_painted(&mut dots, [a, b, c], 0.0, rgb);
            }
        }
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Ok(Marked {
            body,
            keep,
            remove,
            pieces: found.sizes.len(),
            floating: floating.iter().filter(|f| **f).count(),
            triangles: count,
            version,
            dots: Arc::new(dots),
        })
    });
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Floating, inputs: &[crate::selection::SelInput]) -> bool {
    mark(app, k, inputs);
    // 0.05 % to 50 % of the largest piece, on a log scale.
    let format = |v: f64| format!("{:.2} %", 10f64.powf(v));
    form_row(ui, solidcraft_ui_text::tr("ui.scan_floating_below"), |ui| {
        ui_kit::slider(ui, &mut k.log_percent, -1.3..=1.7, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let found = solidcraft_ui_text::format(
                "ui.scan_floating_found",
                &[("floating", &m.floating.to_string()), ("pieces", &m.pieces.to_string()), ("triangles", &m.triangles.to_string())],
            );
            ui_kit::text(ui, &found, ui_kit::Text::Note("scan_floating_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_floating_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, solidcraft_ui_text::tr("ui.scan_floating_hint"), ui_kit::Text::Hint("scan_floating_hint"));
    false
}

/// What OK runs: the floating pieces taken off.
pub fn commands(k: &Floating) -> Result<Vec<(String, Value)>, String> {
    let m = k.shown.as_ref().ok_or("still finding the pieces")?.result.as_ref().map_err(Clone::clone)?;
    if m.floating == 0 {
        return Err(solidcraft_ui_text::tr("ui.scan_floating_none").into());
    }
    let mut p = json!({"body": m.body, "below": k.share()});
    if !m.keep.is_empty() {
        p["keep"] = json!(m.keep);
    }
    if !m.remove.is_empty() {
        p["remove"] = json!(m.remove);
    }
    Ok(vec![("scan.remove_floating".into(), p)])
}

/// The floating pieces marked for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanFloating(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let m = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: m.version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: m.dots.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// The Fill Holes dialog's values.
#[derive(Clone, Debug)]
pub struct Holes {
    /// The largest hole to close, in edges round it.
    pub max_edges: usize,
    /// Each patch shaped to carry the surface across.
    pub fair: bool,
    /// The rings ticked by hand, each by a point of its own, once a click has ticked one.
    pub at: Vec<Vec3>,
    /// Whether the rings are ticked by hand ([`Holes::at`]) rather than by the size limit.
    pub by_hand: bool,
    found: Option<FoundHoles>,
}

impl Default for Holes {
    fn default() -> Self {
        Holes { max_edges: clean::MAX_EDGES, fair: true, at: Vec::new(), by_hand: false, found: None }
    }
}

/// One ring of open edges round a hole or along a piece's rim.
#[derive(Clone, Debug)]
struct Outline {
    points: Vec<Vec3>,
    edges: usize,
    rim: bool,
}

#[derive(Clone, Debug)]
struct FoundHoles {
    revision: u64,
    /// The scan, its rings and the open edges in none; or why not.
    result: Result<(String, Vec<Outline>, usize), String>,
}

/// The ring passing nearest `at`, by its points (as the step finds a ticked ring again).
fn nearest(outlines: &[Outline], at: Vec3) -> Option<usize> {
    let far = |o: &Outline| o.points.iter().map(|p| p.dist(at)).fold(f64::INFINITY, f64::min);
    (0..outlines.len()).min_by(|&a, &b| far(&outlines[a]).total_cmp(&far(&outlines[b])))
}

impl Holes {
    /// Which rings OK closes: those ticked by hand, else the holes within the size limit.
    fn ticked(&self, outlines: &[Outline]) -> Vec<bool> {
        if !self.by_hand {
            return outlines.iter().map(|o| !o.rim && o.edges <= self.max_edges).collect();
        }
        let mut ticked = vec![false; outlines.len()];
        for &a in &self.at {
            if let Some(t) = nearest(outlines, a).and_then(|i| ticked.get_mut(i)) {
                *t = true;
            }
        }
        ticked
    }

    /// Tick or untick ring `i`: from then on the rings are ticked by hand.
    fn turn(&mut self, outlines: &[Outline], i: usize) {
        let mut ticked = self.ticked(outlines);
        if let Some(t) = ticked.get_mut(i) {
            *t = !*t;
        }
        self.by_hand = true;
        self.at = outlines.iter().zip(ticked).filter(|(o, t)| *t && !o.points.is_empty()).map(|(o, _)| o.points[0]).collect();
    }
}

/// Find the holes again when the design has changed.
fn find_holes(app: &SolveApp, k: &mut Holes) {
    if k.found.as_ref().is_some_and(|f| f.revision == app.session.revision) {
        return;
    }
    let result = clean::holes_of(&app.session, None, "scan.fill_holes").map_err(|e| e.to_string()).and_then(|(body, found)| {
        let state = app.session.model.state();
        let (positions, _) = state.body(&body).and_then(|b| b.body.triangle_mesh()).ok_or("the scan is gone")?;
        let outlines = found
            .rings
            .iter()
            .map(|r| Outline {
                points: r.points.iter().filter_map(|&i| positions.get(i as usize).copied()).collect(),
                edges: r.points.len(),
                rim: r.rim,
            })
            .collect();
        Ok((body, outlines, found.unfillable_edges))
    });
    k.found = Some(FoundHoles { revision: app.session.revision, result });
}

/// The Fill Holes dialog's rows.
pub fn holes_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Holes) -> bool {
    find_holes(app, k);
    let before = k.max_edges;
    form_row(ui, solidcraft_ui_text::tr("ui.scan_holes_max"), |ui| ui_kit::number_field(ui, &mut k.max_edges, 3..=5000, 1.0, ""));
    if k.max_edges != before {
        // A new limit ticks by size again.
        k.at.clear();
        k.by_hand = false;
    }
    form_row(ui, solidcraft_ui_text::tr("ui.scan_holes_fair"), |ui| check(ui, &mut k.fair));
    match k.found.as_ref().map(|f| &f.result) {
        Some(Ok((_, outlines, unfillable))) => {
            let within = k.ticked(outlines).iter().filter(|t| **t).count();
            let holes = outlines.iter().filter(|o| !o.rim).count();
            let found = solidcraft_ui_text::format(
                "ui.scan_holes_found",
                &[("within", &within.to_string()), ("holes", &holes.to_string()), ("unfillable", &unfillable.to_string())],
            );
            ui_kit::text(ui, &found, ui_kit::Text::Note("scan_holes_found"));
            if ui_kit::row(ui, "", |ui| ui_kit::button(ui, solidcraft_ui_text::tr("ui.scan_holes_by_size"), ui_kit::Button::Enabled(k.by_hand)))
                .clicked()
            {
                k.at.clear();
                k.by_hand = false;
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_holes_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, solidcraft_ui_text::tr("ui.scan_holes_hint"), ui_kit::Text::Hint("scan_holes_hint"));
    false
}

/// What OK runs: the ticked holes closed.
pub fn holes_commands(k: &Holes) -> Result<Vec<(String, Value)>, String> {
    let (body, outlines, _) = k.found.as_ref().ok_or("still finding the holes")?.result.as_ref().map_err(Clone::clone)?;
    if !k.ticked(outlines).iter().any(|t| *t) {
        return Err(solidcraft_ui_text::tr("ui.scan_holes_none").into());
    }
    let mut p = json!({"body": body, "max_edges": k.max_edges, "fair": k.fair});
    if k.by_hand {
        p["at"] = json!(k.at.iter().map(|a| [a.x, a.y, a.z]).collect::<Vec<_>>());
    }
    Ok(vec![("scan.fill_holes".into(), p)])
}

/// Whether Fill Holes is open: a click in the view ticks the ring nearest it.
pub(crate) fn choosing_holes(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanHoles(_)))
}

/// How near an outline, in points on screen, a click ticks it.
const CLICK_REACH: f32 = 24.0;

/// The ring whose outline passes nearest `at` on screen, within [`CLICK_REACH`].
fn under(outlines: &[Outline], proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, o) in outlines.iter().enumerate() {
        let line: Vec<egui::Pos2> = o.points.iter().filter_map(|p| proj.to_screen(*p)).collect();
        for (j, &a) in line.iter().enumerate() {
            let b = line[(j + 1) % line.len()];
            let d = egui::Rect::from_two_pos(a, b).distance_to_pos(at).max(segment_distance(a, b, at));
            if d <= CLICK_REACH && best.is_none_or(|(_, e)| d < e) {
                best = Some((i, d));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn segment_distance(a: egui::Pos2, b: egui::Pos2, p: egui::Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 { ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// A click in the view while Fill Holes is open: the ring whose outline is nearest is ticked or
/// unticked.
pub(crate) fn hole_click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let Some(Kind::ScanHoles(k)) = app.dialog.as_mut().map(|d| &mut d.kind) else { return };
    let Some(Ok((_, outlines, _))) = k.found.as_ref().map(|f| &f.result) else { return };
    let outlines = outlines.clone();
    if let Some(i) = under(&outlines, proj, at) {
        k.turn(&outlines, i);
    }
}

/// The holes outlined over the picture while the dialog is open: those OK closes in the accent
/// colour, the rest dimmed (a rim dashed).
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanHoles(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, outlines, _))) = k.found.as_ref().map(|f| &f.result) else { return };
    let t = solidcraft_theme::Tokens::get();
    for (o, ticked) in outlines.iter().zip(k.ticked(outlines)) {
        let stroke = if ticked { egui::Stroke::new(2.5, t.accent) } else { egui::Stroke::new(1.0, t.text_dim) };
        let mut line: Vec<egui::Pos2> = o.points.iter().filter_map(|p| proj.to_screen(*p)).collect();
        if line.len() == o.points.len() && line.len() > 1 {
            line.push(line[0]);
            if o.rim && !ticked {
                painter.extend(egui::Shape::dashed_line(&line, stroke, 6.0, 4.0));
            } else {
                painter.add(egui::Shape::line(line, stroke));
            }
        }
    }
}

/// The Clean Up dialog's values: which tools run, in order.
#[derive(Clone, Debug)]
pub struct CleanUp {
    pub repair: bool,
    pub floating: bool,
    pub holes: bool,
    pub reduce: bool,
    /// The share of the triangles Reduce keeps, in percent.
    pub percent: f64,
}

impl Default for CleanUp {
    fn default() -> Self {
        CleanUp { repair: true, floating: true, holes: true, reduce: false, percent: 50.0 }
    }
}

/// The Clean Up dialog's rows: a tick for each tool, in the order they run.
pub fn clean_up_rows(_app: &SolveApp, ui: &mut egui::Ui, k: &mut CleanUp) -> bool {
    let tr = solidcraft_ui_text::tr;
    form_row(ui, tr("cmd.scan_repair"), |ui| check(ui, &mut k.repair));
    form_row(ui, tr("cmd.scan_remove_floating"), |ui| check(ui, &mut k.floating));
    form_row(ui, tr("cmd.scan_fill_holes"), |ui| check(ui, &mut k.holes));
    form_row(ui, tr("cmd.scan_reduce"), |ui| check(ui, &mut k.reduce));
    if k.reduce {
        let format = |v: f64| format!("{v:.0} %");
        form_row(ui, tr("ui.scan_clean_up_reduce"), |ui| {
            ui_kit::slider(ui, &mut k.percent, 5.0..=95.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
        });
    }
    ui_kit::text(ui, tr("ui.scan_clean_up_hint"), ui_kit::Text::Hint("scan_clean_up_hint"));
    false
}

/// What OK runs: one Clean Up, each tool its own step.
pub fn clean_up_commands(k: &CleanUp) -> Result<Vec<(String, Value)>, String> {
    let mut p = json!({"repair": k.repair, "floating": k.floating, "holes": k.holes});
    if k.reduce {
        p["reduce"] = json!((k.percent / 100.0).clamp(0.05, 0.95));
    }
    if !(k.repair || k.floating || k.holes || k.reduce) {
        return Err(solidcraft_ui_text::tr("ui.scan_clean_up_hint").into());
    }
    Ok(vec![("scan.clean_up".into(), p)])
}

/// The Reduce Mesh dialog's values.
#[derive(Clone, Debug)]
pub struct Reduce {
    /// The share of the triangles to keep, in percent.
    pub percent: f64,
    /// The most a collapse may move the surface, in mm; 0 for no such limit.
    pub max_error: f64,
    pub keep_rim: bool,
    /// The scan and its triangles, for the count the share means.
    scan: Option<(u64, Result<(String, usize), String>)>,
}

impl Default for Reduce {
    fn default() -> Self {
        Reduce { percent: 50.0, max_error: 0.0, keep_rim: true, scan: None }
    }
}

/// The Reduce Mesh dialog's rows.
pub fn reduce_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Reduce) -> bool {
    if k.scan.as_ref().is_none_or(|(revision, _)| *revision != app.session.revision) {
        let found = solidcraft_engine::cmd::scan::mesh_body_name(&app.session, None, "scan.reduce").map_err(|e| e.to_string()).and_then(|name| {
            let count = app.session.model.state().body(&name).and_then(|b| b.body.triangle_mesh()).map(|(_, t)| t.len()).ok_or("the scan is gone")?;
            Ok((name, count))
        });
        k.scan = Some((app.session.revision, found));
    }
    let tr = solidcraft_ui_text::tr;
    let format = |v: f64| format!("{v:.0} %");
    form_row(ui, tr("ui.scan_reduce_keep"), |ui| {
        ui_kit::slider(ui, &mut k.percent, 1.0..=99.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    form_row(ui, tr("ui.scan_reduce_error"), |ui| ui_kit::length_field(ui, &mut k.max_error, &units, decimals.max(3)));
    k.max_error = k.max_error.clamp(0.0, 100.0);
    form_row(ui, tr("ui.scan_reduce_rim"), |ui| check(ui, &mut k.keep_rim));
    match k.scan.as_ref().map(|(_, r)| r) {
        Some(Ok((_, count))) => {
            let kept = ((k.percent / 100.0) * *count as f64).round() as usize;
            let note = solidcraft_ui_text::format("ui.scan_reduce_count", &[("now", &count.to_string()), ("kept", &kept.to_string())]);
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_reduce_count"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_reduce_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_reduce_hint"), ui_kit::Text::Hint("scan_reduce_hint"));
    false
}

/// What OK runs: the scan reduced.
pub fn reduce_commands(k: &Reduce) -> Result<Vec<(String, Value)>, String> {
    let (body, _) = k.scan.as_ref().ok_or("still reading the scan")?.1.as_ref().map_err(Clone::clone)?;
    let mut p = json!({"body": body, "ratio": (k.percent / 100.0).clamp(0.01, 0.99), "keep_rim": k.keep_rim});
    if k.max_error > 0.0 {
        p["max_error"] = json!(format!("{} mm", k.max_error));
    }
    Ok(vec![("scan.reduce".into(), p)])
}

/// The clean tools that add a Scan Clean step, or write the scan back with `in_place`.
pub(crate) fn is_clean_step(id: &str) -> bool {
    matches!(id, "scan.remove_floating" | "scan.fill_holes" | "scan.reduce" | "scan.repair" | "scan.denoise" | "scan.remesh")
}

/// With the preference to clean scans in place (the way before), a clean tool's run from the
/// window writes the scan back into its Insert Mesh feature instead of adding a step.
pub(crate) fn in_place(app: &SolveApp, id: &str, params: &mut Value) {
    if app.preferences.scan_clean_in_place
        && is_clean_step(id)
        && let Some(p) = params.as_object_mut()
    {
        p.insert("in_place".into(), json!(true));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A box of `n` × `n` squares a side from `lo` to `hi`, as STL facets.
    fn box_facets(lo: [f64; 3], hi: [f64; 3], n: u32) -> String {
        let mut stl = String::new();
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for high in [false, true] {
                let at = |i: u32, j: u32| {
                    let mut c = lo;
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
        stl
    }

    /// A block with a speck beside it: the dialog marks the speck's 12 triangles and OK takes
    /// them off.
    #[test]
    fn the_speck_is_marked_and_taken_off() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("specked.stl");
        let stl =
            format!("solid s\n{}{}endsolid s\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12), box_facets([50.0, 0.0, 0.0], [51.0, 1.0, 1.0], 1));
        std::fs::write(&path, stl).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.remove_floating");
        h.frames(2);
        let marks = gpu_marks(&h.app).expect("the speck marked");
        assert_eq!(marks.triangles.len(), 12 * crate::scan_dots::DOT_SIZE, "its 12 triangles painted");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, _) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(positions.iter().all(|p| p.x <= 30.0), "the speck is gone");
    }

    /// A click on the speck keeps it: nothing is left to take off; a click on the block marks
    /// the block to go instead, and OK sends the point.
    #[test]
    fn a_click_turns_a_piece_over() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("specked.stl");
        let stl =
            format!("solid s\n{}{}endsolid s\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12), box_facets([50.0, 0.0, 0.0], [51.0, 1.0, 1.0], 1));
        std::fs::write(&path, stl).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let body = h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
        h.app.start("scan.remove_floating");
        let click = |h: &mut crate::scenario::Harness, at: [f64; 3]| {
            let Some(d) = h.app.dialog.as_mut() else { panic!("open") };
            d.inputs[0].items.push(solidcraft_engine::Sel::Face { body: body.clone(), index: 0, point: Vec3::new(at[0], at[1], at[2]) });
            h.frames(2);
        };
        click(&mut h, [50.5, 0.5, 1.0]);
        let Some(Kind::ScanFloating(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("open") };
        assert!(super::commands(k).is_err(), "the speck is kept: nothing floats");
        click(&mut h, [15.0, 10.0, 10.0]);
        let Some(Kind::ScanFloating(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("open") };
        let cmds = super::commands(k).unwrap();
        assert_eq!(cmds[0].1["keep"].as_array().map(Vec::len), Some(1), "{}", cmds[0].1);
        assert_eq!(cmds[0].1["remove"].as_array().map(Vec::len), Some(1), "{}", cmds[0].1);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        let state = h.app.session.model.state();
        let (positions, _) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(positions.iter().all(|p| p.x >= 50.0), "only the speck is left");
    }

    /// Taking the speck off is computed beside the window, as a feature's OK is: a real scan's
    /// millions of triangles took a minute on the window thread. The dialog stays, saying it is
    /// computing, nothing changes until the result is in, and then it is applied.
    #[test]
    fn taking_the_speck_off_is_computed_beside_the_window() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("specked.stl");
        let stl =
            format!("solid s\n{}{}endsolid s\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12), box_facets([50.0, 0.0, 0.0], [51.0, 1.0, 1.0], 1));
        std::fs::write(&path, stl).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.apply_off_thread = Some(std::time::Duration::ZERO);
        let (finish, gate) = std::sync::mpsc::channel();
        h.app.apply_gate = Some(std::sync::Arc::new(std::sync::Mutex::new(gate)));
        h.app.start("scan.remove_floating");
        h.frames(2);
        let before = h.app.session.doc.clone();
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        assert!(h.app.applying.is_some(), "computing beside the window");
        h.frames(2);
        assert!(std::sync::Arc::ptr_eq(&before, &h.app.session.doc) && h.app.dialog.is_some(), "nothing applied while it computes");
        finish.send(()).unwrap();
        for _ in 0..500 {
            if h.app.applying.is_none() {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        h.frames(1);
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, _) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(positions.iter().all(|p| p.x <= 30.0), "the speck is gone");
        // One undo step, as on the window thread.
        h.app.run("edit.undo", json!({})).unwrap();
        assert!(std::sync::Arc::ptr_eq(&before, &h.app.session.doc));
    }

    /// A plate with a square hole: the dialog finds the hole, and OK closes it.
    #[test]
    fn the_hole_is_found_and_closed() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid p\n");
        for y in 0..10 {
            for x in 0..10 {
                if (4..=5).contains(&x) && (4..=5).contains(&y) {
                    continue;
                }
                let p = |i: i32, j: i32| [x + i, y + j, 0];
                for tri in [[p(0, 0), p(1, 0), p(1, 1)], [p(0, 0), p(1, 1), p(0, 1)]] {
                    stl += "facet normal 0 0 0\nouter loop\n";
                    for [a, b, c] in tri {
                        stl += &format!("vertex {a} {b} {c}\n");
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        let path = h.dir.join("plate.stl");
        std::fs::write(&path, stl + "endsolid p\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.fill_holes");
        h.frames(2);
        let Some(Kind::ScanHoles(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Fill Holes dialog") };
        let Some(Ok((_, outlines, _))) = k.found.as_ref().map(|f| &f.result) else { panic!("holes found") };
        assert_eq!(outlines.iter().filter(|o| !o.rim).map(|o| o.edges).collect::<Vec<_>>(), vec![8], "the hole, not the rim");
        assert!(outlines.iter().any(|o| o.rim), "the rim is outlined too");
        let outlines = outlines.clone();
        let hole = outlines.iter().position(|o| !o.rim).unwrap();
        // Unticked by a click, nothing is closed; ticked again, it is closed by its point.
        let Some(Kind::ScanHoles(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) else { panic!("open") };
        k.turn(&outlines, hole);
        assert!(super::holes_commands(k).is_err(), "nothing ticked");
        k.turn(&outlines, hole);
        let cmds = super::holes_commands(k).unwrap();
        assert_eq!(cmds[0].1["at"].as_array().map(Vec::len), Some(1), "{}", cmds[0].1);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (_, triangles) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(triangles.len() > 192, "patched: {}", triangles.len());
    }

    /// A finely cut block reduced to a tenth through the dialog keeps its shape.
    #[test]
    fn the_dialog_reduces_a_block() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("block.stl");
        std::fs::write(&path, format!("solid b\n{}endsolid b\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12))).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.reduce");
        h.frames(2);
        if let Some(Kind::ScanReduce(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.percent = 10.0;
        }
        h.frames(1);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, triangles) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(triangles.len() <= 173, "a tenth of 1728: {}", triangles.len());
        assert!(positions.iter().all(|p| p.x >= -1e-4 && p.x <= 30.0 + 1e-4 && p.z <= 10.0 + 1e-4), "the block's shape");
    }
}
