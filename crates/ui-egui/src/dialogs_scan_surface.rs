//! Fit Surface (`scan.fit_surface`): click the scan where the surface goes; the area round the
//! click is marked on the scan and the surface fitted to it drawn over the picture as lines across
//! it, with how close it comes, as the radius, the control points and the smoothing change. OK
//! keeps it.
//!
//! Edit Surface (`scan.edit_surface`, after QuickSurface's): a surface Fit Surface or New Free Form
//! made, one face or a Free Form's network of faces sharing whole sides, edited in the view.
//!
//! - Points on the surface (the default): a dot on the surface for each control point, spread
//!   evenly over its face. A dot dragged takes the surface through where it is let go: onto the scan under the
//!   pointer with Snap to scan on (the default), else in the plane of the screen. The other dots
//!   stay where they are (the surface still runs through them) or, with Neighbours set to Follow,
//!   go with it smoothly. Faces that share a side move together and stay tangent where they met
//!   smoothly. Snap All to Scan puts every dot on the scan; Fit to Scan fits every face to it.
//! - Control net: a handle at each control point, moved in the plane of the screen or onto the
//!   scan, with Refit, a Falloff (a soft selection) and Slide along the surface, and the net tools
//!   on a row or column of one face.
//! - Add Face: click an open edge, then the new face's two far corners (a click near a point of
//!   the Free Form is that point): the face shares the edge, meets the face it grows from
//!   tangentially and is fitted to the scan. Corners other than a grid's (QuickSurface's red
//!   points) are marked.
//!
//! The scan under the surface is coloured by how far it is from it as it is edited, against a
//! tolerance: within it the analysis's good colour, above the surface its bad colour, below it the
//! deep end of the curvature map. Surface Check runs on the faces as shown, so a fold a drag makes
//! is seen as it is made. OK keeps every face as one undo step; Reset Points puts them back.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::surface::network::{self, Dot, Joins, Side};
use solidcraft_engine::cmd::scan::surface::{self as edit, Covered, EditableSurface, Line};
use solidcraft_engine::geom::bspline::NurbsSurface;
use solidcraft_engine::geom::prelude::*;
use solidcraft_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit::{self, choice_row};

/// What a drag refits (0 nothing): the ring round the point dragged, or every point not moved by
/// hand.
pub const REFIT_AROUND: usize = 1;
pub const REFIT_ALL: usize = 2;

/// How many rings round the dragged point Refit Around frees, and a dot dragged moves.
const RING: usize = 2;

/// A handle's half size, and how near the pointer must be pressed to take it, in pixels.
const HANDLE: f32 = 4.0;
const GRAB: f32 = 10.0;

/// How near the pointer must be to an open edge to take it for Add Face, in pixels.
const EDGE_GRAB: f32 = 24.0;

/// The most scan points a drag's preview refits to: the full set (as the command uses) once it
/// is let go.
const PREVIEW_POINTS: usize = 2_500;

/// A face drawn as this many lines each way (fewer on each face of a network), each sampled at
/// this many points.
const LINES: usize = 10;
const NETWORK_LINES: usize = 4;
const SAMPLES: usize = 40;

/// Cells each way Surface Check uses on the surface shown (it runs on every frame of a drag).
const CHECK_GRID: usize = 24;

/// What a dragged handle does: moves its point through space, or slides it along the surface.
pub const DRAG_MOVE: usize = 0;
pub const DRAG_SLIDE: usize = 1;

/// What the handles are: points on the surface, or the control net behind it.
pub const HANDLES_DOTS: usize = 0;
pub const HANDLES_NET: usize = 1;

/// What the dots round one dragged do: stay where they are, or follow it smoothly.
pub const NEIGHBOURS_HOLD: usize = 0;
pub const NEIGHBOURS_FOLLOW: usize = 1;

/// The deviation colouring's tolerance to begin with, mm, and how many tolerances out the colours
/// are deepest.
const TOLERANCE: f64 = 0.1;
const DEEPEST: f64 = 8.0;

/// The colouring's id among the clouds the viewport draws, and a dot's size on the screen.
const SHADE_CLOUD_ID: u64 = u64::MAX - 7;
const SHADE_PIXELS: f32 = 4.0;

thread_local! {
    static SHADE_VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The Edit Surface dialog's values.
#[derive(Clone, Debug)]
pub struct EditSurface {
    /// What the handles are: [`HANDLES_DOTS`] or [`HANDLES_NET`].
    pub handles: usize,
    /// Where a dragged point goes: free in the plane of the screen ([`SNAP_OFF`]), onto the scan
    /// under the pointer ([`SNAP_POINTER`]) or onto the scan's nearest place ([`SNAP_NEAREST`]).
    pub snap: usize,
    /// The scan is met no farther than this from where a point is dragged (mm; 0, no limit), so a
    /// point dragged over an opening stays where it is put.
    pub snap_reach: f64,
    /// What a dragged dot's neighbours do: [`NEIGHBOURS_HOLD`] or [`NEIGHBOURS_FOLLOW`].
    pub neighbours: usize,
    /// The row or column chosen: a drag of one of its points moves it all, and the net tools act on it.
    line: Option<Line>,
    /// Why the last tool did nothing.
    note: Option<String>,
    /// What a drag refits: 0 nothing, [`REFIT_AROUND`] or [`REFIT_ALL`].
    pub refit: usize,
    /// The surface body to edit, when it was named (opened from the history); else the selected
    /// one, or the one made last.
    target: Option<String>,
    loaded: Option<Result<Box<Loaded>, String>>,
    /// The faces as edited so far, how they join, and whether their shape has changed (a face
    /// added, points doubled, a row split): then every face is written back.
    faces: Vec<NurbsSurface>,
    joins: Arc<Joins>,
    reshaped: bool,
    /// The network's points (the control net, a point two faces share once) as edited so far, and
    /// those moved by hand.
    net: Vec<Vec3>,
    moved: Vec<bool>,
    /// The faces shown (as edited, or a drag's preview of them), their network points and dots.
    shown: Vec<NurbsSurface>,
    shown_net: Vec<Vec3>,
    shown_dots: Vec<Dot>,
    /// The scan's distance from the faces shown: root mean square and largest.
    fit: Option<(f64, f64)>,
    chosen: Option<usize>,
    /// How far Move takes the chosen point along the way the surface faces, in mm.
    pub along: f64,
    /// The points within this of one moved go with it, less the further they are (mm; 0, none).
    pub falloff: f64,
    /// What a dragged handle does: [`DRAG_MOVE`] or [`DRAG_SLIDE`].
    pub drag_mode: usize,
    /// How far Slide takes the chosen point (or its line) along u and along v, in mm.
    pub slide: [f64; 2],
    /// The deviation colouring's tolerance, mm.
    pub tolerance: f64,
    /// Surface Check on each face shown.
    check: Vec<edit::SurfaceCheck>,
    drag: Option<Drag>,
    /// Add Face, while it is on: the open edge taken and the far corners clicked.
    adding: Option<Adding>,
    /// Delete Face is on: a click on a face takes it away.
    deleting: bool,
    /// The scan under the faces and how far each point is from them, coloured.
    shade: Option<Box<Shading>>,
    /// The faces' shape changed since the scan under them was looked for.
    restudy: bool,
    /// The edits so far, last last: each the faces, the points moved by hand and whether the
    /// shape had changed before it, for Undo Last Change.
    history: Vec<Before>,
}

/// The dialog's faces before an edit.
#[derive(Clone, Debug)]
struct Before {
    faces: Vec<NurbsSurface>,
    moved: Vec<bool>,
    reshaped: bool,
}

/// How many edits Undo Last Change goes back, at most.
const HISTORY: usize = 100;

impl Default for EditSurface {
    fn default() -> Self {
        EditSurface {
            handles: HANDLES_DOTS,
            snap: SNAP_POINTER,
            snap_reach: 0.0,
            neighbours: NEIGHBOURS_HOLD,
            line: None,
            note: None,
            refit: REFIT_AROUND,
            target: None,
            loaded: None,
            faces: Vec::new(),
            joins: Arc::new(Joins::default()),
            reshaped: false,
            net: Vec::new(),
            moved: Vec::new(),
            shown: Vec::new(),
            shown_net: Vec::new(),
            shown_dots: Vec::new(),
            fit: None,
            chosen: None,
            along: 1.0,
            falloff: 0.0,
            drag_mode: DRAG_MOVE,
            slide: [1.0, 0.0],
            tolerance: TOLERANCE,
            check: Vec::new(),
            drag: None,
            adding: None,
            deleting: false,
            shade: None,
            restudy: false,
            history: Vec::new(),
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

/// Add Face as it goes: the open edge taken (a face and its side), the far corners clicked, and
/// the new face's points across.
#[derive(Clone, Debug, Default, PartialEq)]
struct Adding {
    side: Option<(usize, Side)>,
    corners: Vec<Vec3>,
}

/// The scan under the faces: where each point lies on them, how far it is, and the coloured dots.
#[derive(Clone, Debug)]
struct Shading {
    shade: network::Shade,
    values: Vec<f32>,
    version: u64,
    dots: Arc<Vec<u8>>,
    /// The tolerance and colours the dots were coloured with.
    key: (u64, [[u8; 3]; 3]),
}

#[derive(Clone, Debug)]
struct Drag {
    index: usize,
    /// Where the point (or dot) was when the drag began: the depth it keeps.
    from: Vec3,
    /// The points that go with it (its line, when one is chosen and holds it), and where each was.
    with: Vec<(usize, Vec3)>,
    /// Sliding: the surface as the drag began and the dragged point's place on it.
    slide: Option<Box<SlideFrom>>,
    /// A dot on the surface, not a control point.
    dot: bool,
    /// Alt-dragging a dot on an open edge: the open edges it is on (a face grows from the one the
    /// drag leaves most squarely), and where the pointer has taken it.
    grow: Vec<(usize, Side)>,
    to: Option<Vec3>,
}

#[derive(Clone, Debug)]
struct SlideFrom {
    surface: NurbsSurface,
    /// The dragged point's Greville place `(u, v)` and the surface's point there.
    at: (f64, f64),
    on: Vec3,
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
            let mut dots = Vec::with_capacity(found.marked.len() * crate::scan_dots::DOT_SIZE);
            for at in &found.marked {
                crate::scan_dots::cloud_dot(&mut dots, &crate::scan_dots::Dot { at: *at, rgb, ..crate::scan_dots::Dot::default() });
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
            crate::dialogs_surface_check::fold_warning(ui, found.trouble);
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
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
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
        Some(Ok(l)) if l.revision == revision => {
            if k.restudy {
                study(app, k);
            }
            return;
        }
        Some(Err(_)) => return,
        _ => {}
    }
    let cmd = "scan.edit_surface";
    let found = edit::editable(&app.session, k.target.as_deref(), cmd).map_err(|e| e.to_string());
    if let (Some(Ok(l)), Ok(target)) = (&mut k.loaded, &found)
        && l.target.feature == target.feature
        && l.target.faces == target.faces
    {
        l.revision = revision;
        return;
    }
    let loaded = found.map(|target| Box::new(Loaded { target, covered: None, sample: Arc::new(Vec::new()), revision }));
    let faces = loaded.as_ref().ok().map(|l| l.target.faces.clone());
    k.loaded = Some(loaded);
    if let Some(faces) = faces {
        reset_to(k, faces);
        study(app, k);
    }
}

/// The scan under the faces looked for again (as loaded, or a face added): for refits, the
/// distance and the colouring.
fn study(app: &SolveApp, k: &mut EditSurface) {
    k.restudy = false;
    let cmd = "scan.edit_surface";
    let found = edit::shade(&app.session, None, &k.faces, cmd).ok().filter(|(_, s)| !s.points.is_empty());
    if let Some(Ok(l)) = k.loaded.as_mut() {
        l.covered = found.as_ref().map(|(scan, s)| Covered { scan: scan.clone(), points: Arc::new(s.points.iter().map(|p| p.at).collect()) });
        let all = l.covered.as_ref().map(|c| c.points.as_slice()).unwrap_or_default();
        l.sample = Arc::new(all.iter().step_by(all.len() / PREVIEW_POINTS + 1).copied().collect());
    }
    k.shade = found.map(|(_, shade)| Box::new(Shading { shade, values: Vec::new(), version: 0, dots: Arc::new(Vec::new()), key: (0, [[0; 3]; 3]) }));
    let shown = k.shown.clone();
    show_faces(k, shown, false, None);
}

/// The dialog started again from `faces`: nothing moved, nothing chosen.
fn reset_to(k: &mut EditSurface, faces: Vec<NurbsSurface>) {
    k.joins = Arc::new(network::joins(&faces).unwrap_or_default());
    k.net = network::network_points(&faces, &k.joins);
    k.moved = vec![false; k.net.len()];
    k.chosen = None;
    k.line = None;
    k.drag = None;
    k.reshaped = false;
    k.adding = None;
    k.history.clear();
    k.faces = faces.clone();
    show_faces(k, faces, false, None);
}

/// The faces as they are, kept for Undo Last Change before an edit changes them.
fn remember(k: &mut EditSurface) {
    if k.history.len() >= HISTORY {
        k.history.remove(0);
    }
    k.history.push(Before { faces: k.faces.clone(), moved: k.moved.clone(), reshaped: k.reshaped });
}

/// The last edit undone: the faces as they were before it.
fn undo_last(k: &mut EditSurface) {
    let Some(before) = k.history.pop() else { return };
    let restudy = before.faces.len() != k.faces.len();
    k.joins = Arc::new(network::joins(&before.faces).unwrap_or_default());
    k.net = network::network_points(&before.faces, &k.joins);
    k.moved = before.moved;
    k.reshaped = before.reshaped;
    k.chosen = None;
    k.line = None;
    k.drag = None;
    k.faces = before.faces.clone();
    k.restudy |= restudy;
    show_faces(k, before.faces, false, None);
}

/// The faces' shape changed (a face added, points doubled): `faces` as edited from now on, what
/// was moved kept moved where it still is.
fn reshaped_to(k: &mut EditSurface, faces: Vec<NurbsSurface>) {
    remember(k);
    let moved_at: Vec<Vec3> = k.net.iter().zip(&k.moved).filter(|(_, m)| **m).map(|(p, _)| *p).collect();
    k.joins = Arc::new(network::joins(&faces).unwrap_or_default());
    k.net = network::network_points(&faces, &k.joins);
    k.moved = k.net.iter().map(|p| moved_at.iter().any(|q| q.dist(*p) < 1e-9)).collect();
    k.reshaped = true;
    k.chosen = None;
    k.faces = faces.clone();
    k.restudy = true;
    show_faces(k, faces, false, None);
}

/// The one face, when the surface is one face whose points are its network's (not a network).
fn base(k: &EditSurface) -> Option<&NurbsSurface> {
    match k.faces.as_slice() {
        [one] if one.count()[0] * one.count()[1] == k.net.len() => Some(one),
        _ => None,
    }
}

/// Whether the surface is a network of several faces.
fn is_network(k: &EditSurface) -> bool {
    k.faces.len() > 1
}

/// The net changed as a whole by `change` (a net tool), what was moved kept moved where it still is;
/// `then` the line to choose after.
fn reshape(k: &mut EditSurface, change: impl FnOnce(&NurbsSurface) -> Result<(NurbsSurface, Option<Line>), edit::NetError>) -> Result<(), String> {
    if is_network(k) {
        return Err(tr("ui.scan_edit_one_face").to_owned());
    }
    let current = k.shown.first().cloned().or_else(|| base(k).cloned()).ok_or("no surface")?;
    let (made, then) = change(&current).map_err(|e| e.to_string())?;
    reshaped_to(k, vec![made.clone()]);
    k.line = then;
    k.chosen = then.and_then(|l| l.points(made.count()).first().copied());
    Ok(())
}

fn loaded(k: &EditSurface) -> Option<&Loaded> {
    k.loaded.as_ref()?.as_deref().ok()
}

/// Show `surface` (the one face), as [`show_faces`].
fn show(k: &mut EditSurface, surface: NurbsSurface, dragging: bool) {
    show_faces(k, vec![surface], dragging, None);
}

/// Show `faces`, with how far the scan is from them: the scan's places on the faces `touched`
/// marks found again (on all with `None`), and the colouring with them.
fn show_faces(k: &mut EditSurface, faces: Vec<NurbsSurface>, dragging: bool, touched: Option<&[bool]>) {
    let _ = dragging;
    k.shown_net = network::network_points(&faces, &k.joins);
    k.shown_dots = network::dots(&faces, &k.joins);
    k.check = faces.iter().filter_map(|f| edit::check_surface(f, CHECK_GRID)).collect();
    k.fit = None;
    if let Some(sh) = k.shade.as_mut() {
        network::shade_values(&mut sh.shade, &faces, touched, &mut sh.values);
        let (mut squares, mut max, mut n) = (0.0, 0.0f64, 0usize);
        for v in sh.values.iter().map(|v| f64::from(*v).abs()).filter(|v| v.is_finite()) {
            squares += v * v;
            max = max.max(v);
            n += 1;
        }
        if n > 0 {
            k.fit = Some(((squares / n as f64).sqrt(), max));
        }
        sh.key = (0, [[0; 3]; 3]);
    }
    k.shown = faces;
    recolour(k);
}

/// The points round `around` a dot's drag moves, in every face that has them.
fn ring_of(k: &EditSurface, around: &[usize]) -> Vec<bool> {
    network::region(&k.faces, &k.joins, around, network::DOT_RING)
}

/// The surface with the points `moves` put where they go, those round them carried along as the
/// Falloff says, and, as Refit says, the points round them or all those not moved by hand fitted
/// to the scan again with them (and those carried) held. The one face's net.
fn preview(k: &EditSurface, moves: &[(usize, Vec3)]) -> Option<NurbsSurface> {
    let (net, carried) = edit::with_falloff(&k.net, moves, &k.moved, k.falloff);
    refitted(k, net, &moves.iter().map(|m| m.0).collect::<Vec<_>>(), &carried)
}

/// The surface with its net at `net` (the points `now` moved by hand and those `carried` with
/// them), refitted as Refit says.
fn refitted(k: &EditSurface, net: Vec<Vec3>, now: &[usize], carried: &[bool]) -> Option<NurbsSurface> {
    let l = loaded(k)?;
    let mut moved = k.moved.clone();
    for &index in now {
        *moved.get_mut(index)? = true;
    }
    let surface = edit::with_points(base(k)?, &net)?;
    let points = l.sample.as_slice();
    let count = surface.count();
    let kept = |i: usize| moved.get(i).copied().unwrap_or(false) || carried.get(i).copied().unwrap_or(false);
    let held: Vec<bool> = match k.refit {
        REFIT_AROUND => {
            let mut near = vec![false; net.len()];
            for &index in now {
                for (n, r) in edit::around(count, index % count[0], index / count[0], RING).into_iter().enumerate() {
                    near[n] |= r;
                }
            }
            near.iter().enumerate().map(|(i, near)| !near || kept(i)).collect()
        }
        REFIT_ALL => (0..net.len()).map(kept).collect(),
        _ => return Some(surface),
    };
    if points.is_empty() {
        return Some(surface);
    }
    Some(edit::refit(&surface, points, &held).unwrap_or(surface))
}

/// The network with its points `moves` put where they go (control net handles on a network): the
/// points next to them across smooth seams kept in line, so the faces stay tangent.
fn net_moved(k: &EditSurface, moves: &[(usize, Vec3)]) -> Option<Vec<NurbsSurface>> {
    let mut places = k.net.clone();
    for &(i, to) in moves {
        *places.get_mut(i)? = to;
    }
    let faces = network::with_network_points(&k.faces, &k.joins, &places)?;
    let moved: Vec<usize> = moves.iter().map(|m| m.0).collect();
    let mut free = vec![false; places.len()];
    for t in &k.joins.smooth {
        let (e, a, b) = (t.e, t.a, t.b);
        if moved.contains(&e) || moved.contains(&a) || moved.contains(&b) {
            for p in [a, b] {
                free[p] = !moved.contains(&p);
            }
        }
    }
    if !free.iter().any(|f| *f) {
        return Some(faces);
    }
    let moves = network::Moves { free: &free, goals: &[], tangent: 40.0, even: 0.0, bend: 0.0, also_smooth: &[] };
    network::moved(&faces, &k.joins, &moves).or(Some(faces))
}

/// The faces with the dot `index` put at `to`: the dots round it held or following as
/// Neighbours says, and with Refit Around the scan under the ring fitted too.
fn dot_preview(k: &EditSurface, moves: &[(usize, Vec3)]) -> Option<Vec<NurbsSurface>> {
    let around: Vec<usize> = moves.iter().map(|m| m.0).collect();
    let hold = k.neighbours == NEIGHBOURS_HOLD;
    let extra = match (k.refit, loaded(k)) {
        (REFIT_AROUND | REFIT_ALL, Some(l)) if !hold && !l.sample.is_empty() => {
            let free = ring_of(k, &around);
            let touched: Vec<bool> = k.joins.of.iter().map(|ids| ids.iter().any(|p| free[*p])).collect();
            let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
            network::feet(&k.faces, &l.sample, reach, &touched)
        }
        _ => Vec::new(),
    };
    network::dots_moved(&k.faces, &k.joins, moves, network::DOT_RING, hold, &extra)
}

/// The faces a move of the points `around` changes: those that hold a point within the ring.
fn touched_by(k: &EditSurface, around: &[usize]) -> Vec<bool> {
    let free = ring_of(k, around);
    k.joins.of.iter().map(|ids| ids.iter().any(|p| free[*p])).collect()
}

/// The points `chosen` slid along the surface shown by `step` for each, those round them carried
/// as the Falloff says: the net, and which points were carried.
fn slid_net(k: &EditSurface, surface: &NurbsSurface, chosen: &[usize], step: impl Fn(usize) -> (f64, f64)) -> Option<(Vec<Vec3>, Vec<bool>)> {
    let others: Vec<bool> = k.moved.iter().enumerate().map(|(i, m)| *m && !chosen.contains(&i)).collect();
    let mut weights = edit::falloff_weights(&k.net, chosen, &others, k.falloff);
    for &c in chosen {
        *weights.get_mut(c)? = 1.0;
    }
    let net = edit::slid(surface, &weights, step)?;
    let carried = weights.iter().enumerate().map(|(i, w)| *w > 0.0 && !chosen.contains(&i)).collect();
    Some((net, carried))
}

/// The points that go with the one chosen: its line when one is chosen and holds it, else itself.
fn going_with(k: &EditSurface, index: usize) -> Vec<usize> {
    match (k.line, base(k).map(NurbsSurface::count)) {
        (Some(l), Some(c)) if l.points(c).contains(&index) => l.points(c),
        _ => vec![index],
    }
}

/// The chosen point (or its line) slid by the lengths typed, as a drag let go there would.
fn slide_typed(k: &mut EditSurface, index: usize) {
    let Some(surface) = k.shown.first().cloned().filter(|_| base(k).is_some()) else { return };
    let chosen = going_with(k, index);
    let steps = edit::slide_steps(&surface, k.slide);
    let Some((net, carried)) = slid_net(k, &surface, &chosen, |i| steps.get(i).copied().unwrap_or((0.0, 0.0))) else { return };
    if let Some(made) = refitted(k, net, &chosen, &carried) {
        show(k, made, false);
        commit(k, &chosen);
    }
}

/// The faces shown kept as edited, the points `by_hand` counted as moved by hand.
fn commit(k: &mut EditSurface, by_hand: &[usize]) {
    remember(k);
    k.faces = k.shown.clone();
    k.net = k.shown_net.clone();
    for &i in by_hand {
        if let Some(m) = k.moved.get_mut(i) {
            *m = true;
        }
    }
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
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let name = match k.faces.as_slice() {
        [one] => format!("{} ({} × {})", l.target.body, one.count()[0], one.count()[1]),
        many => crate::i18n::format("ui.scan_edit_faces_value", &[("body", &l.target.body), ("faces", &many.len().to_string())]),
    };
    form_row(ui, crate::i18n::tr_label("Surface"), |ui| ui_kit::text(ui, &name, ui_kit::Text::Cell));
    let odd = k.joins.irregular().count();
    if odd > 0 {
        let line = crate::i18n::format("ui.scan_edit_irregular_value", &[("count", &odd.to_string())]);
        form_row(ui, tr("ui.scan_edit_irregular"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Report("scan_edit_irregular", false)));
    }
    let kinds = [tr("ui.scan_edit_handles_dots"), tr("ui.scan_edit_handles_net")];
    let before = k.handles;
    choice_row(ui, tr("ui.scan_edit_handles"), "scan_edit_handles", &mut k.handles, &kinds);
    if k.handles != before {
        k.chosen = None;
        k.line = None;
        k.adding = None;
    }
    let dots = k.handles != HANDLES_NET;
    if l.covered.is_some() {
        let snaps = [tr("ui.scan_edit_snap_off"), tr("ui.scan_edit_snap_pointer"), tr("ui.scan_edit_snap_nearest")];
        choice_row(ui, tr("ui.scan_edit_snap"), "scan_edit_snap", &mut k.snap, &snaps);
        if k.snap != SNAP_OFF {
            form_row(ui, tr("ui.scan_edit_snap_reach"), |ui| ui_kit::length_field(ui, &mut k.snap_reach, &units, decimals));
            k.snap_reach = k.snap_reach.max(0.0);
        }
    } else {
        ui_kit::text(ui, tr("ui.scan_edit_no_scan"), ui_kit::Text::Hint("scan_edit_no_scan"));
    }
    if dots {
        let ways = [tr("ui.scan_edit_neighbours_hold"), tr("ui.scan_edit_neighbours_follow")];
        choice_row(ui, tr("ui.scan_edit_neighbours"), "scan_edit_neighbours", &mut k.neighbours, &ways);
        if k.neighbours == NEIGHBOURS_FOLLOW && l.covered.is_some() {
            let labels = [tr("ui.scan_edit_refit_off"), tr("ui.scan_edit_refit_around")];
            let mut refit = k.refit.min(1);
            choice_row(ui, tr("ui.scan_edit_refit"), "scan_edit_refit", &mut refit, &labels);
            k.refit = refit;
        }
    } else if !is_network(k) {
        if l.covered.is_some() {
            let labels = [tr("ui.scan_edit_refit_off"), tr("ui.scan_edit_refit_around"), tr("ui.scan_edit_refit_all")];
            choice_row(ui, tr("ui.scan_edit_refit"), "scan_edit_refit", &mut k.refit, &labels);
        }
        form_row(ui, tr("ui.scan_edit_falloff"), |ui| ui_kit::length_field(ui, &mut k.falloff, &units, decimals));
        k.falloff = if k.falloff.is_finite() { k.falloff.max(0.0) } else { 0.0 };
        let modes = [tr("ui.scan_edit_drag_move"), tr("ui.scan_edit_drag_slide")];
        choice_row(ui, tr("ui.scan_edit_drag"), "scan_edit_drag", &mut k.drag_mode, &modes);
    }
    let moved = k.moved.iter().filter(|m| **m).count().to_string();
    form_row(ui, tr("ui.scan_edit_moved"), |ui| ui_kit::text(ui, &moved, ui_kit::Text::Cell));
    // How far the scan is from the surface, and its colouring against the tolerance.
    if let Some((rms, max)) = k.fit {
        let fit = crate::i18n::format(
            "ui.scan_edit_distance_value",
            &[("rms", &crate::prefs::show_mm(app, rms, 1)), ("max", &crate::prefs::show_mm(app, max, 1))],
        );
        ui_kit::text(ui, &format!("{}: {fit}", tr("ui.scan_edit_distance")), ui_kit::Text::Note("scan_edit_distance"));
    }
    if k.shade.is_some() {
        form_row(ui, tr("ui.scan_edit_tolerance"), |ui| ui_kit::length_field(ui, &mut k.tolerance, &units, decimals));
        k.tolerance = if k.tolerance.is_finite() && k.tolerance > 0.0 { k.tolerance } else { TOLERANCE };
        recolour(k);
        let counted: Vec<f32> = k.shade.as_ref().map(|sh| sh.values.iter().copied().filter(|v| v.is_finite()).collect()).unwrap_or_default();
        if !counted.is_empty() {
            let within = counted.iter().filter(|v| f64::from(v.abs()) <= k.tolerance).count() as f64 / counted.len() as f64;
            let line = crate::i18n::format(
                "ui.scan_edit_within",
                &[("within", &format!("{:.0} %", 100.0 * within)), ("tolerance", &crate::prefs::show_mm(app, k.tolerance, 1))],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_edit_within"));
        }
    }
    // Surface Check on the faces as shown: a fold a drag makes is seen as it is made.
    if !k.check.is_empty() {
        let (folds, pinches, crossings) = k.check.iter().fold((0, 0, 0), |(f, p, c), x| (f + x.folds, p + x.pinches, c + x.crossings));
        if folds + pinches + crossings == 0 {
            form_row(ui, tr("ui.scan_edit_check"), |ui| ui_kit::text(ui, tr("ui.scan_edit_check_clean"), ui_kit::Text::Cell));
        } else {
            let line = crate::dialogs_surface_check::trouble(folds, pinches, crossings);
            form_row(ui, tr("ui.scan_edit_check"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Report("scan_edit_check", true)));
        }
    }
    let edited = k.moved.iter().any(|m| *m) || k.reshaped || k.faces != l.target.faces;
    let can_undo = !k.history.is_empty();
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_undo"), ui_kit::Button::Enabled(can_undo))).clicked() && can_undo {
        undo_last(k);
    }
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_reset"), ui_kit::Button::Enabled(edited))).clicked() {
        reset_to(k, l.target.faces.clone());
        k.restudy = true;
    }
    whole_tools(app, ui, k, &l);
    if !dots && !is_network(k) {
        let count = base(k).map_or([0, 0], NurbsSurface::count);
        net_tools(ui, k, count);
    }
    chosen_rows(app, ui, k, &units, decimals);
    if let Some(note) = &k.note {
        ui_kit::text(ui, note, ui_kit::Text::Report("scan_edit_note", true));
    }
    let hint = match (&k.adding, dots) {
        _ if k.deleting => tr("ui.scan_edit_delete_face_hint"),
        (Some(a), _) if a.side.is_none() => tr("ui.scan_edit_add_face_edge"),
        (Some(_), _) => tr("ui.scan_edit_add_face_corners"),
        (None, true) => tr("ui.scan_edit_hint_dots"),
        (None, false) => tr("ui.scan_edit_hint"),
    };
    ui_kit::text(ui, hint, ui_kit::Text::Hint("scan_edit_hint"));
    false
}

/// The tools on the whole surface: Add Face, Increase Resolution, Snap All to Scan and Fit to
/// Scan, one to a row.
fn whole_tools(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface, l: &Loaded) {
    let mut outcome: Option<Result<(), String>> = None;
    let on = k.adding.is_some();
    let scan = l.covered.is_some();
    let style = if on { ui_kit::Button::SmallSelected } else { ui_kit::Button::Enabled(scan) };
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_add_face"), style)).clicked() && scan {
        k.adding = if on { None } else { Some(Adding::default()) };
        k.deleting = false;
        k.chosen = None;
        k.line = None;
    }
    if is_network(k) || k.deleting {
        let style = if k.deleting { ui_kit::Button::SmallSelected } else { ui_kit::Button::Normal };
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_delete_face"), style)).clicked() {
            k.deleting = !k.deleting;
            k.adding = None;
            k.chosen = None;
        }
    }
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_smooth_all"), ui_kit::Button::Normal)).clicked() {
        let goals = match (loaded(k).and_then(|l| l.covered.as_ref()), k.snap != SNAP_OFF) {
            (Some(c), true) => {
                let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
                network::feet(&k.faces, &c.points, reach, &vec![true; k.faces.len()])
            }
            _ => Vec::new(),
        };
        outcome = Some(match network::smoothed(&k.faces, &k.joins, &goals, 0.5) {
            Some(faces) => {
                show_faces(k, faces, false, None);
                commit(k, &[]);
                Ok(())
            }
            None => Err(tr("ui.scan_edit_nothing").to_owned()),
        });
    }
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_refine"), ui_kit::Button::Normal)).clicked() {
        outcome = Some(match network::refined(&k.faces) {
            Ok(made) => {
                reshaped_to(k, made);
                Ok(())
            }
            Err(e) => Err(e.to_string()),
        });
    }
    if scan {
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_snap_all"), ui_kit::Button::Normal)).clicked() {
            outcome = Some(snap_all(app, k));
        }
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_fit_all"), ui_kit::Button::Normal)).clicked() {
            outcome = Some(fit_all(k));
        }
    }
    match outcome {
        Some(Err(why)) => k.note = Some(why),
        Some(Ok(())) => k.note = None,
        None => {}
    }
}

/// Every dot put on the scan's nearest place (within the snap distance), the surface through them all.
fn snap_all(app: &SolveApp, k: &mut EditSurface) -> Result<(), String> {
    let scan = loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.scan.clone())).ok_or_else(|| tr("ui.scan_edit_no_scan").to_owned())?;
    let (_, on) = solidcraft_engine::cmd::scan::curve::projector(&app.session, Some(&scan), "scan.edit_surface").map_err(|e| e.to_string())?;
    let reach = if k.snap_reach > 0.0 { k.snap_reach } else { f64::INFINITY };
    let moves: Vec<(usize, Vec3)> =
        k.shown_dots.iter().enumerate().filter_map(|(i, d)| on.nearest(d.at).filter(|(_, dist)| *dist <= reach).map(|(q, _)| (i, q))).collect();
    if moves.is_empty() {
        return Err(tr("ui.scan_edit_no_scan").to_owned());
    }
    let faces = network::dots_moved(&k.faces, &k.joins, &moves, 0, true, &[]).ok_or_else(|| tr("ui.scan_edit_nothing").to_owned())?;
    show_faces(k, faces, false, None);
    let all: Vec<usize> = moves.iter().map(|m| m.0).collect();
    commit(k, &all);
    Ok(())
}

/// Every face fitted to the scan under it, the faces held tangent where they met smoothly; the
/// corners where faces meet in other than a grid's way, and the network's own corners, held.
fn fit_all(k: &mut EditSurface) -> Result<(), String> {
    let points = loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.points.clone())).ok_or_else(|| tr("ui.scan_edit_no_scan").to_owned())?;
    let mut faces = k.faces.clone();
    let reach = faces.iter().map(edit::default_reach).fold(1.0, f64::max);
    let held: Vec<usize> = k.joins.corners.iter().filter(|c| c.irregular() || (c.border && c.faces == 1)).map(|c| c.point).collect();
    let free: Vec<bool> = (0..k.net.len()).map(|p| !held.contains(&p) && !k.moved[p]).collect();
    let all = vec![true; faces.len()];
    for _ in 0..3 {
        let goals = network::feet(&faces, &points, reach, &all);
        let moves = network::Moves { free: &free, goals: &goals, tangent: 40.0, even: 0.0, bend: 1e-4, also_smooth: &[] };
        match network::moved(&faces, &k.joins, &moves) {
            Some(next) => faces = next,
            None => break,
        }
    }
    show_faces(k, faces, false, None);
    commit(k, &[]);
    Ok(())
}

/// The point chosen (clicked or last dragged): where it goes, typed, or a distance along the way
/// the surface faces; with the control net of one face, a slide along it.
fn chosen_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface, units: &str, decimals: u8) {
    let _ = app;
    let dots = k.handles == HANDLES_DOTS;
    let Some(i) = k.chosen.filter(|i| *i < k.net.len()) else { return };
    let which = match k.joins.at.get(i).and_then(|a| a.first()) {
        Some(&(f, at)) => {
            let nu = k.faces.get(f).map_or(1, |s| s.count()[0]);
            if is_network(k) { format!("{} · [{}, {}]", f + 1, at % nu, at / nu) } else { format!("[{}, {}]", at % nu, at / nu) }
        }
        None => String::new(),
    };
    form_row(ui, tr("ui.scan_edit_point"), |ui| ui_kit::text(ui, &which, ui_kit::Text::Cell));
    let p = if dots { k.shown_dots.get(i).map_or(k.net[i], |d| d.at) } else { k.net[i] };
    let mut xyz = [p.x, p.y, p.z];
    let mut typed = false;
    for (axis, v) in ["X", "Y", "Z"].into_iter().zip(&mut xyz) {
        form_row(ui, axis, |ui| {
            let r = ui_kit::length_field(ui, v, units, decimals);
            typed |= r.changed();
            r
        });
    }
    if typed {
        move_to(k, i, Vec3::new(xyz[0], xyz[1], xyz[2]));
    }
    form_row(ui, tr("ui.scan_edit_along"), |ui| ui_kit::length_field(ui, &mut k.along, units, decimals));
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_move"), ui_kit::Button::Normal)).clicked() {
        let normal = if dots {
            k.shown_dots.get(i).and_then(|d| network::dot_normal(&k.shown, d))
        } else {
            k.joins.at.get(i).and_then(|a| a.first()).and_then(|&(f, at)| k.shown.get(f).and_then(|s| edit::normal_at(s, at)))
        };
        if let Some(n) = normal {
            move_to(k, i, p + n * k.along);
        }
    }
    if dots || base(k).is_none() {
        return;
    }
    form_row(ui, tr("ui.scan_edit_slide_u"), |ui| ui_kit::length_field(ui, &mut k.slide[0], units, decimals));
    form_row(ui, tr("ui.scan_edit_slide_v"), |ui| ui_kit::length_field(ui, &mut k.slide[1], units, decimals));
    for v in &mut k.slide {
        if !v.is_finite() {
            *v = 0.0;
        }
    }
    let can = k.slide != [0.0, 0.0];
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_slide"), ui_kit::Button::Enabled(can))).clicked() && can {
        slide_typed(k, i);
    }
}

/// The net tools (SurfaceCraft's, after QuickSurface's), on the line chosen: choose the row or
/// column of the point chosen; split it, copy it beside itself, take it away, crease it or make it
/// smooth. Each changes the dialog's net; OK keeps it. One face only.
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
    if let Some(Err(why)) = outcome {
        k.note = Some(why);
    } else if outcome.is_some() {
        k.note = None;
    }
}

/// Point (or dot) `index` put at `to`, as a drag let go there would: refitted as Refit says, and
/// kept.
fn move_to(k: &mut EditSurface, index: usize, to: Vec3) {
    let made = if k.handles == HANDLES_DOTS {
        dot_preview(k, &[(index, to)])
    } else if base(k).is_some() {
        preview(k, &[(index, to)]).map(|s| vec![s])
    } else {
        net_moved(k, &[(index, to)])
    };
    if let Some(faces) = made {
        show_faces(k, faces, false, None);
        commit(k, &[index]);
    }
}

/// What OK runs: the net as edited; every face, once a face was added or a net's shape changed,
/// or for a network.
pub fn commands(k: &EditSurface) -> Result<Vec<(String, Value)>, String> {
    let l = k.loaded.as_ref().ok_or("still loading the surface")?.as_ref().map_err(Clone::clone)?;
    if k.faces == l.target.faces && !k.reshaped {
        return Err(crate::i18n::tr("ui.scan_edit_nothing").into());
    }
    let net_of = |s: &NurbsSurface| {
        let points: Vec<[f64; 3]> = edit::control_points(s).iter().map(|p| [p.x, p.y, p.z]).collect();
        json!({"count": s.count(), "knots": [s.knots()[0].clone(), s.knots()[1].clone()], "degree": s.degree(), "points": points})
    };
    match (k.faces.as_slice(), k.reshaped) {
        ([one], false) => {
            let points: Vec<[f64; 3]> = edit::control_points(one).iter().map(|p| [p.x, p.y, p.z]).collect();
            Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "points": points}))])
        }
        ([one], true) => Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "net": net_of(one)}))]),
        (many, _) => Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "faces": many.iter().map(net_of).collect::<Vec<_>>()}))]),
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

/// Whether the dialog shows handles (the next primary press on one takes it), or takes clicks
/// for Add Face.
pub(crate) fn editing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if loaded(k).is_some())
}

/// The surface body Edit Surface is editing: the model does not draw it while the dialog draws
/// it as edited.
pub(crate) fn edited_body(app: &SolveApp) -> Option<&str> {
    match app.dialog.as_ref().map(|d| &d.kind) {
        Some(Kind::ScanEditSurface(k)) => loaded(k).map(|l| l.target.body.as_str()),
        _ => None,
    }
}

/// Whether a handle is being dragged: the drag neither selects nor turns the view.
pub(crate) fn holds_pointer(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if k.drag.is_some())
}

/// The handle under `at` on the screen, if any.
fn handle_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<usize> {
    let near = |i: usize, p: Vec3| proj.to_screen(p).map(|s| (i, s.distance(at)));
    let found: Vec<(usize, f32)> = if k.handles == HANDLES_DOTS {
        k.shown_dots.iter().enumerate().filter_map(|(i, d)| near(i, d.at)).collect()
    } else {
        k.shown_net.iter().enumerate().filter_map(|(i, p)| near(i, *p)).collect()
    };
    found.into_iter().filter(|(_, d)| *d <= GRAB).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i)
}

/// The face under `at` on the screen: the one whose outline on the screen holds it.
fn face_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<usize> {
    k.shown.iter().position(|s| {
        let outline: Vec<egui::Pos2> = [Side::FirstRow, Side::LastColumn, Side::LastRow, Side::FirstColumn]
            .into_iter()
            .enumerate()
            .flat_map(|(n, side)| {
                let mut on = side_on_screen(proj, s, side);
                // Round the face in one direction: its last row and first column run backwards.
                if n >= 2 {
                    on.reverse();
                }
                on
            })
            .collect();
        inside(&outline, at)
    })
}

/// Whether `p` lies inside the closed polygon `outline` (even-odd).
fn inside(outline: &[egui::Pos2], p: egui::Pos2) -> bool {
    let mut odd = false;
    for (i, a) in outline.iter().enumerate() {
        let b = outline[(i + 1) % outline.len()];
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (b.x - a.x) * (p.y - a.y) / (b.y - a.y) {
            odd = !odd;
        }
    }
    odd
}

/// The open edge nearest `at` on the screen, within [`EDGE_GRAB`].
fn open_edge_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<(usize, Side)> {
    k.joins
        .open
        .iter()
        .filter_map(|&(f, side)| {
            let s = k.shown.get(f)?;
            let curve: Vec<egui::Pos2> =
                (0..=SAMPLES).filter_map(|j| side.at(s, j as f64 / SAMPLES as f64).and_then(|p| proj.to_screen(p))).collect();
            let d = curve.windows(2).map(|w| distance_to_segment(at, w[0], w[1])).fold(f32::INFINITY, f32::min);
            Some(((f, side), d))
        })
        .filter(|(_, d)| *d <= EDGE_GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(e, _)| e)
}

fn distance_to_segment(p: egui::Pos2, a: egui::Pos2, b: egui::Pos2) -> f32 {
    let (ab, ap) = (b - a, p - a);
    let t = if ab.length_sq() > 0.0 { (ap.dot(ab) / ab.length_sq()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

/// A click at `at`: chooses the handle under it, to type where its point goes; with Add Face on,
/// takes an open edge, then the new face's far corners.
pub(crate) fn choose(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    if dialog(app).is_some_and(|k| k.adding.is_some()) {
        add_face_click(app, proj, at);
        return;
    }
    if let Some(k) = dialog(app).filter(|k| k.deleting) {
        if let Some(face) = face_at(k, proj, at) {
            match network::without_face(&k.faces, face) {
                Ok(faces) => {
                    reshaped_to(k, faces);
                    k.note = None;
                    k.deleting = is_network(k);
                }
                Err(e) => k.note = Some(e.to_string()),
            }
        }
        return;
    }
    if let Some(k) = dialog(app)
        && let Some(index) = handle_at(k, proj, at)
    {
        k.chosen = Some(index);
    }
}

/// Add Face's click: the open edge under it, then a far corner (a point of the surface near it is
/// that point; else the scan under it, else level with the edge in the plane of the screen). The
/// second corner adds the face.
fn add_face_click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::Face { body, point, .. } => Some((body, point)),
        _ => None,
    });
    let Some(k) = dialog(app) else { return };
    let Some(adding) = k.adding.as_mut() else { return };
    let Some((face, side)) = adding.side else {
        let taken = open_edge_at(k, proj, at);
        if let (Some(a), Some(e)) = (k.adding.as_mut(), taken) {
            a.side = Some(e);
            k.note = None;
        }
        return;
    };
    let scan = loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.scan.clone()));
    // A point of the surface near the click is welded to.
    let near_point = k
        .shown_net
        .iter()
        .filter_map(|p| proj.to_screen(*p).map(|s| (*p, s.distance(at))))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p);
    let on_scan = hit.filter(|(body, _)| scan.as_deref() == Some(body.as_str())).map(|(_, p)| p);
    let level = || {
        let ids = k.joins.side(&k.shown, face, side);
        let anchor = *k.shown_net.get(*ids.first()?)?;
        let (o, d) = proj.ray(at);
        let n = proj.cam.back();
        let den = d.dot(n);
        (den.abs() > 1e-12).then(|| o + d * ((anchor - o).dot(n) / den))
    };
    let Some(place) = near_point.or(on_scan).or_else(level) else { return };
    let corners = {
        let Some(a) = k.adding.as_mut() else { return };
        a.corners.push(place);
        a.corners.clone()
    };
    if corners.len() < 2 {
        return;
    }
    let rows = rows_for(k, face, side);
    let reach = (k.snap_reach > 0.0).then_some(k.snap_reach);
    let faces = k.faces.clone();
    let made =
        solidcraft_engine::cmd::scan::free_form::added_face(&app.session, scan.as_deref(), &faces, face, side, [corners[0], corners[1]], rows, reach);
    let Some(k) = dialog(app) else { return };
    k.adding = Some(Adding::default());
    match made {
        Ok((_, faces)) => {
            reshaped_to(k, faces);
            k.note = None;
        }
        Err(e) => k.note = Some(e.to_string()),
    }
}

/// A face dragged out of an open edge: the edge the drag left most squarely, its far corners (the
/// edge's ends moved as the dot was), and its points across. `None` for a drag that went nowhere.
fn grown_far(k: &EditSurface, d: &Drag) -> Option<(usize, Side, [Vec3; 2], usize)> {
    let delta = d.to? - d.from;
    if delta.len() < 1e-6 {
        return None;
    }
    let ends = |&(f, side): &(usize, Side)| -> Option<(Vec3, Vec3)> {
        let ids = k.joins.side(&k.faces, f, side);
        Some((*k.net.get(*ids.first()?)?, *k.net.get(*ids.last()?)?))
    };
    let &(face, side) = d.grow.iter().max_by(|a, b| {
        let square = |e: &(usize, Side)| ends(e).and_then(|(p, q)| (q - p).normalized()).map_or(0.0, |t| 1.0 - t.dot(delta).abs() / delta.len());
        square(a).total_cmp(&square(b))
    })?;
    let (p, q) = ends(&(face, side))?;
    Some((face, side, [p + delta, q + delta], rows_for(k, face, side)))
}

/// A new face's points across: as many as the face it grows from has that way.
fn rows_for(k: &EditSurface, face: usize, side: Side) -> usize {
    let Some(s) = k.faces.get(face) else { return 4 };
    match side {
        Side::FirstRow | Side::LastRow => s.count()[1],
        Side::FirstColumn | Side::LastColumn => s.count()[0],
    }
    .clamp(4, 12)
}

/// A primary press at `at`: takes the handle under it, if any. With `copy` (Alt held) a point on
/// an edge of the net has its edge copied beside itself first and the copy taken, to drag the
/// surface out (QuickSurface's Alt-drag; the control net of one face).
pub(crate) fn grab(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, copy: bool) -> bool {
    let Some(k) = dialog(app) else { return false };
    if k.adding.is_some() || k.deleting {
        return false;
    }
    let Some(mut index) = handle_at(k, proj, at) else { return false };
    // A drag starts from the faces as shown (a refit the last drag made is kept).
    k.faces = k.shown.clone();
    k.net = k.shown_net.clone();
    if k.handles == HANDLES_DOTS {
        let Some(dot) = k.shown_dots.get(index).copied() else { return false };
        // Alt on a dot of an open edge drags a new face out of that edge (QuickSurface's Alt-drag).
        let grow: Vec<(usize, Side)> = if copy {
            k.joins.open.iter().copied().filter(|&(f, side)| k.joins.side(&k.faces, f, side).contains(&index)).collect()
        } else {
            Vec::new()
        };
        k.drag = Some(Drag { index, from: dot.at, with: vec![(index, dot.at)], slide: None, dot: true, grow, to: None });
        k.chosen = Some(index);
        return true;
    }
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
    // Sliding: the surface as it is and the point's place on it, which the pointer then moves.
    let slide = (k.drag_mode == DRAG_SLIDE && base(k).is_some())
        .then(|| {
            let surface = k.shown.first()?.clone();
            let (u, v) = *edit::greville_places(&surface).get(index)?;
            let on = surface.eval(u, v).ok()?;
            Some(Box::new(SlideFrom { surface, at: (u, v), on }))
        })
        .flatten();
    k.drag = Some(Drag { index, from: k.net[index], with, slide, dot: false, grow: Vec::new(), to: None });
    k.chosen = Some(index);
    true
}

/// The handle dragged to `to` on the screen.
pub(crate) fn drag_to(app: &mut SolveApp, proj: &crate::viewport::Proj, to: egui::Pos2) {
    // Sliding: the point goes to the surface's place under the pointer (in the plane of the screen
    // through where it was on the surface), the others going with it the same way in u and v.
    if let Some(from) = dialog(app).and_then(|k| k.drag.as_ref()?.slide.clone()) {
        let (o, d) = proj.ray(to);
        let n = proj.cam.back();
        let den = d.dot(n);
        if den.abs() <= 1e-12 {
            return;
        }
        let Some(f) = edit::foot(&from.surface, o + d * ((from.on - o).dot(n) / den)) else { return };
        let step = (f.u - from.at.0, f.v - from.at.1);
        let Some(k) = dialog(app) else { return };
        let chosen: Vec<usize> = k.drag.as_ref().map(|d| d.with.iter().map(|(i, _)| *i).collect()).unwrap_or_default();
        if let Some((net, carried)) = slid_net(k, &from.surface, &chosen, |_| step)
            && let Some(surface) = refitted(k, net, &chosen, &carried)
        {
            show(k, surface, true);
        }
        return;
    }
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
    // Growing a face: only where the pointer has taken the edge, until it is let go.
    if !drag.grow.is_empty() {
        if let Some(d) = k.drag.as_mut() {
            d.to = Some(place);
        }
        return;
    }
    let around: Vec<usize> = moves.iter().map(|m| m.0).collect();
    let made = if drag.dot {
        dot_preview(k, &moves)
    } else if base(k).is_some() {
        preview(k, &moves).map(|s| vec![s])
    } else {
        net_moved(k, &moves)
    };
    if let Some(faces) = made {
        let touched = touched_by(k, &around);
        show_faces(k, faces, true, Some(&touched));
    }
}

/// The drag let go: the faces as previewed are kept and the point counts as moved by hand; a
/// face dragged out of an open edge is added, out to where the edge was taken.
pub(crate) fn release(app: &mut SolveApp) {
    let grown = dialog(app).and_then(|k| {
        let d = k.drag.as_ref().filter(|d| !d.grow.is_empty())?;
        Some((grown_far(k, d)?, loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.scan.clone())), k.faces.clone(), k.snap_reach))
    });
    if let Some(((face, side, far, rows), scan, faces, reach)) = grown {
        let reach = (reach > 0.0).then_some(reach);
        let made = solidcraft_engine::cmd::scan::free_form::added_face(&app.session, scan.as_deref(), &faces, face, side, far, rows, reach);
        let Some(k) = dialog(app) else { return };
        k.drag = None;
        match made {
            Ok((_, faces)) => {
                reshaped_to(k, faces);
                k.note = None;
            }
            Err(e) => k.note = Some(e.to_string()),
        }
        return;
    }
    let Some(k) = dialog(app) else { return };
    let Some(drag) = k.drag.take() else { return };
    let changed = k.shown_net.len() == k.net.len() && (k.shown_net != k.net || k.shown != k.faces);
    if changed {
        let by_hand: Vec<usize> = drag.with.iter().map(|(i, _)| *i).collect();
        commit(k, &by_hand);
    }
    // Measured again on every covered point, as the command will.
    let shown = k.shown.clone();
    show_faces(k, shown, false, None);
}

/// The colouring of the scan under the surface, as a cloud for the GPU viewport.
pub(crate) fn gpu_shade(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanEditSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let sh = k.shade.as_ref().filter(|s| s.version != 0)?;
    Some(shade_cloud(SHADE_CLOUD_ID, sh.version, sh.dots.clone()))
}

/// A colouring's dots as a cloud for the GPU viewport, under `id`.
pub(crate) fn shade_cloud(id: u64, version: u64, dots: Arc<Vec<u8>>) -> GpuCloud {
    GpuCloud {
        id,
        version,
        dots,
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: SHADE_PIXELS },
    }
}

/// The colouring built again when the values, the tolerance or the theme's colours have changed.
fn recolour(k: &mut EditSurface) {
    let tolerance = k.tolerance;
    let Some(sh) = k.shade.as_mut() else { return };
    let key = (tolerance.to_bits(), crate::scan_deviation::colours());
    if sh.key == key && sh.version != 0 {
        return;
    }
    sh.dots = Arc::new(shade_dots(&sh.shade, &sh.values, tolerance, key.1));
    sh.version = next_shade_version();
    sh.key = key;
}

/// A new version for a colouring's dots (each is uploaded again).
pub(crate) fn next_shade_version() -> u64 {
    SHADE_VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    })
}

/// The scan's points under the surface as coloured dots: each by how far it is from the surface,
/// against `tolerance`.
pub(crate) fn shade_dots(shade: &network::Shade, values: &[f32], tolerance: f64, colours: [[u8; 3]; 3]) -> Vec<u8> {
    let mut out = Vec::with_capacity(shade.points.len() * crate::scan_dots::DOT_SIZE);
    for (p, v) in shade.points.iter().zip(values) {
        let Some(s) = solidcraft_scan_scale(*v, tolerance) else { continue };
        let dot = crate::scan_dots::Dot { at: p.at.to_f32(), value: *v, rgb: crate::scan_deviation::colour(colours, s) };
        crate::scan_dots::cloud_dot(&mut out, &dot);
    }
    out
}

/// Where a distance sits on the colouring's scale, -1 to 1 (0 within the tolerance).
fn solidcraft_scan_scale(v: f32, tolerance: f64) -> Option<f64> {
    let v = f64::from(v);
    if !v.is_finite() {
        return None;
    }
    if v.abs() <= tolerance {
        return Some(0.0);
    }
    Some((v.signum() * (v.abs() - tolerance) / (tolerance * (DEEPEST - 1.0))).clamp(-1.0, 1.0))
}

/// A side of `s` as screen points, where they are in front of the camera.
fn side_on_screen(proj: &crate::viewport::Proj, s: &NurbsSurface, side: Side) -> Vec<egui::Pos2> {
    (0..=SAMPLES).filter_map(|j| side.at(s, j as f64 / SAMPLES as f64).and_then(|p| proj.to_screen(p))).collect()
}

/// The faces shown, their sides, the handles (dots on the surface or the control net), the
/// corners that are not a grid's, and Add Face's edge and guides, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let t = crate::theme::Tokens::get();
    if let Some(Kind::ScanFitSurface(k)) = app.dialog.as_ref().map(|d| &d.kind)
        && let Some(Ok((found, _, _))) = k.shown.as_ref().map(|s| &s.result)
    {
        draw_lines(painter, proj, &found.surface, egui::Stroke::new(1.5, t.surface_accent));
        return;
    }
    let Some(Kind::ScanEditSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    if k.shown.is_empty() {
        return;
    }
    let lines = if k.shown.len() > 1 { NETWORK_LINES } else { LINES };
    for face in &k.shown {
        draw_lines_n(painter, proj, face, lines, egui::Stroke::new(1.0, t.surface_accent.gamma_multiply(0.8)));
    }
    // The sides: shared ones thin, the open edge bold (Add Face's edge under the pointer, or the
    // one taken, in the accent).
    let pointer = painter.ctx().input(|i| i.pointer.hover_pos());
    let hovered = match (&k.adding, pointer) {
        (Some(a), _) if a.side.is_some() => a.side,
        (Some(_), Some(at)) => open_edge_at(k, proj, at),
        _ => None,
    };
    for seam in &k.joins.seams {
        if let Some(s) = k.shown.get(seam.a) {
            painter.add(egui::Shape::line(side_on_screen(proj, s, seam.a_side), egui::Stroke::new(1.5, t.surface_accent)));
        }
    }
    for &(f, side) in &k.joins.open {
        let Some(s) = k.shown.get(f) else { continue };
        let (width, colour) = if hovered == Some((f, side)) {
            (4.0, t.accent)
        } else if k.adding.is_some() {
            (2.5, t.accent_soft)
        } else {
            (2.0, t.surface_accent)
        };
        painter.add(egui::Shape::line(side_on_screen(proj, s, side), egui::Stroke::new(width, colour)));
    }
    // Delete Face: the face under the pointer outlined in the error colour.
    if k.deleting
        && let Some(f) = pointer.and_then(|at| face_at(k, proj, at))
        && let Some(s) = k.shown.get(f)
    {
        for side in [Side::FirstRow, Side::LastRow, Side::FirstColumn, Side::LastColumn] {
            painter.add(egui::Shape::line(side_on_screen(proj, s, side), egui::Stroke::new(3.0, t.error)));
        }
    }
    // Add Face: from the edge's ends to the corners clicked and the pointer.
    if let Some(a) = &k.adding
        && let Some((f, side)) = a.side
    {
        let ids = k.joins.side(&k.shown, f, side);
        let ends: Vec<egui::Pos2> =
            [ids.first(), ids.last()].into_iter().flatten().filter_map(|p| k.shown_net.get(*p)).filter_map(|p| proj.to_screen(*p)).collect();
        let mut far: Vec<egui::Pos2> = a.corners.iter().filter_map(|p| proj.to_screen(*p)).collect();
        if let Some(p) = pointer {
            far.push(p);
        }
        let guide = egui::Stroke::new(1.5, t.warning);
        if let ([e0, e1], [c0, rest @ ..]) = (ends.as_slice(), far.as_slice()) {
            // Each far corner goes with the end of the edge it lies nearer, as the face is made.
            let (n0, n1) = if e0.distance(*c0) <= e1.distance(*c0) { (*e0, *e1) } else { (*e1, *e0) };
            painter.line_segment([n0, *c0], guide);
            if let Some(c1) = rest.first() {
                painter.line_segment([n1, *c1], guide);
                painter.line_segment([*c0, *c1], guide);
            }
        }
        for c in a.corners.iter().filter_map(|p| proj.to_screen(*p)) {
            painter.circle_filled(c, HANDLE + 1.0, t.warning);
        }
    }
    // A face being dragged out of an open edge: the edge and where its far side goes.
    if let Some(d) = &k.drag
        && let Some((f, side, far, _)) = grown_far(k, d)
    {
        let ids = k.joins.side(&k.shown, f, side);
        let ends: Vec<egui::Pos2> =
            [ids.first(), ids.last()].into_iter().flatten().filter_map(|p| k.shown_net.get(*p)).filter_map(|p| proj.to_screen(*p)).collect();
        let far: Vec<egui::Pos2> = far.iter().filter_map(|p| proj.to_screen(*p)).collect();
        if let ([e0, e1], [f0, f1]) = (ends.as_slice(), far.as_slice()) {
            let guide = egui::Stroke::new(1.5, t.warning);
            painter.line_segment([*e0, *f0], guide);
            painter.line_segment([*e1, *f1], guide);
            painter.line_segment([*f0, *f1], guide);
        }
    }
    let moved_fill = crate::theme::model_color(t.analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8));
    let fill_of = |i: usize, on_line: bool| {
        if k.chosen == Some(i) {
            t.accent
        } else if on_line {
            t.accent_soft
        } else if k.moved.get(i).copied().unwrap_or(false) {
            moved_fill
        } else {
            t.text
        }
    };
    if k.handles == HANDLES_DOTS {
        if k.adding.is_none() {
            for (i, d) in k.shown_dots.iter().enumerate() {
                let Some(s) = proj.to_screen(d.at) else { continue };
                painter.circle_filled(s, HANDLE, fill_of(i, false));
                painter.circle_stroke(s, HANDLE, egui::Stroke::new(1.0, t.panel));
            }
        }
    } else {
        // The control net, face by face.
        let screen: Vec<Option<egui::Pos2>> = k.shown_net.iter().map(|p| proj.to_screen(*p)).collect();
        let net = egui::Stroke::new(1.0, t.text.gamma_multiply(0.45));
        for (face, ids) in k.shown.iter().zip(&k.joins.of) {
            let [nu, nv] = face.count();
            for j in 0..nv {
                for i in 0..nu {
                    let here = ids.get(j * nu + i).and_then(|p| screen.get(*p).copied().flatten());
                    for next in [(i + 1 < nu).then(|| j * nu + i + 1), (j + 1 < nv).then(|| (j + 1) * nu + i)].into_iter().flatten() {
                        if let (Some(a), Some(b)) = (here, ids.get(next).and_then(|p| screen.get(*p).copied().flatten())) {
                            painter.line_segment([a, b], net);
                        }
                    }
                }
            }
        }
        if k.adding.is_none() {
            let count = base(k).map(NurbsSurface::count);
            for (i, s) in screen.iter().enumerate() {
                let Some(s) = s else { continue };
                let on_line = k.line.zip(count).is_some_and(|(l, c)| l.points(c).contains(&i));
                let rect = egui::Rect::from_center_size(*s, egui::vec2(2.0 * HANDLE, 2.0 * HANDLE));
                painter.rect_filled(rect, 1.0, fill_of(i, on_line));
                painter.rect_stroke(rect, 1.0, egui::Stroke::new(1.0, t.panel), egui::StrokeKind::Outside);
            }
        }
    }
    // Corners where faces meet other than as a grid's: the surface is less smooth there.
    for c in k.joins.irregular() {
        if let Some(s) = k.shown_net.get(c.point).and_then(|p| proj.to_screen(*p)) {
            painter.circle_stroke(s, HANDLE + 3.0, egui::Stroke::new(2.0, t.error));
        }
    }
    // Where Surface Check finds the surface shown folds, pinches or crosses itself.
    for (trouble, at) in k.check.iter().flat_map(|c| &c.places) {
        if let Some(s) = proj.to_screen(*at) {
            crate::dialogs_surface_check::mark(painter, s, trouble.name());
        }
    }
}

/// Lines across `surface`, `lines` each way, over the picture.
fn draw_lines_n(painter: &egui::Painter, proj: &crate::viewport::Proj, surface: &NurbsSurface, lines: usize, stroke: egui::Stroke) {
    let [[u0, u1], [v0, v1]] = surface.domain();
    let at = |i: usize, n: usize, a: f64, b: f64| a + (b - a) * i as f64 / n as f64;
    for along_u in [true, false] {
        for i in 1..lines {
            let curve: Option<Vec<egui::Pos2>> = (0..=SAMPLES)
                .map(|j| {
                    let (u, v) =
                        if along_u { (at(j, SAMPLES, u0, u1), at(i, lines, v0, v1)) } else { (at(i, lines, u0, u1), at(j, SAMPLES, v0, v1)) };
                    surface.eval(u, v).ok().and_then(|p| proj.to_screen(p))
                })
                .collect();
            if let Some(curve) = curve {
                painter.add(egui::Shape::line(curve, stroke));
            }
        }
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

    /// The dialog's handles set to the control net, a dragged point free of the scan.
    fn control_net(h: &mut crate::scenario::Harness) {
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.handles = HANDLES_NET;
            k.snap = SNAP_OFF;
        }
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
        control_net(&mut h);
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
            let n = k.shown.first().and_then(|s| edit::normal_at(s, j)).unwrap();
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

    /// With a Falloff, a point moved carries its neighbours along, less the further they are; Slide
    /// carries a point along the surface; a drag with Drag set to Slide does so too; and the
    /// surface's check line turns from clean to folds when a point is pulled back past its
    /// neighbours. OK keeps the net as edited.
    #[test]
    fn falloff_and_slide_move_points_and_the_check_sees_a_fold() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [30, 30, 8], "radius": 25, "controls": 6})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        control_net(&mut h);
        assert!(kind(&h).check.iter().all(edit::SurfaceCheck::is_clean) && !kind(&h).check.is_empty(), "a fitted dome is clean");
        let start = kind(&h).net.clone();
        let k = 2 * 6 + 2;
        let step = start[k].dist(start[k + 1]);
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.refit = 0;
            e.falloff = 2.0 * step;
            move_to(e, k, start[k] + Vec3::new(0.0, 0.0, 6.0));
            let next = e.net[k + 1].z - start[k + 1].z;
            assert!(next > 1.0 && next < 5.0, "a neighbour goes part of the way: {next}");
            assert!(!e.moved[k + 1], "carried, not moved by hand");
            assert!((e.net[k + 4] - start[k + 4]).len() < 1e-9 || start[k + 4].dist(start[k]) < 2.0 * step);

            // Slide 3 mm along u: the point keeps to the surface, about 3 mm on.
            e.falloff = 0.0;
            let j = 3 * 6 + 2;
            let before = e.net[j];
            e.slide = [3.0, 0.0];
            slide_typed(e, j);
            let d = e.net[j] - before;
            assert!(e.moved[j] && d.len() > 2.0 && d.len() < 4.0, "{d:?}");

            // A column pulled back past the next one folds the surface, and the check says so.
            let fold: Vec<(usize, Vec3)> = (0..6).map(|r| (r * 6 + 3, e.net[r * 6 + 1] - Vec3::new(4.0, 0.0, 0.0))).collect();
            let folded = preview(e, &fold).unwrap();
            show(e, folded, false);
            assert!(e.check.iter().any(|c| c.folds > 0), "{:?}", e.check);
            let back = base(e).cloned().unwrap();
            let net = e.net.clone();
            show(e, edit::with_points(&back, &net).unwrap(), false);
        }

        // A drag in Slide mode moves the point along the surface, toward the pointer.
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.drag_mode = DRAG_SLIDE;
            e.chosen = None;
            e.line = None;
        }
        let j = 3 * 6 + 3;
        let from = kind(&h).shown_net[j];
        let to = from + Vec3::new(0.0, 5.0, 0.0);
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 6})).unwrap();
        h.frames(2);
        let after = kind(&h).clone();
        assert!(after.moved[j], "slid by hand");
        let d = after.net[j] - from;
        assert!(d.y > 2.0 && d.x.abs() < 1.5, "along the surface toward the pointer: {d:?}");

        let edited = after.net.clone();
        h.frames(2);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let kept = edit::control_points(&edit::editable(&h.app.session, Some("Surface1"), "test").unwrap().surface);
        assert!(kept.iter().zip(&edited).all(|(a, b)| a.dist(*b) < 1e-6), "the net as edited, kept");
    }

    /// Points on the surface, the default: a dot dragged up in the front view takes the surface
    /// through where it is let go while the other dots stay; the scan under the surface is
    /// coloured by how far it is from it, and the colouring changes with the drag; OK keeps it.
    #[test]
    fn a_point_on_the_surface_dragged_takes_the_surface_through_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 6})).unwrap();
        h.step(&json!({"view_anim": "front"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert_eq!((kind(&h).handles, kind(&h).snap), (HANDLES_DOTS, SNAP_POINTER), "points on the surface, snapped to the scan, by default");
        let cloud = gpu_shade(&h.app).expect("the scan under the surface coloured");
        assert!(cloud.dots.len() > 100 * crate::scan_dots::DOT_SIZE, "{}", cloud.dots.len());
        let (rms, _) = kind(&h).fit.expect("the distance");
        assert!(rms < 0.05, "a Free Form lies on the dome: {rms}");
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.snap = SNAP_OFF;
        }
        let before = kind(&h).shown_dots.clone();
        let i = 2 * 6 + 2;
        let (from, to) = (before[i].at, before[i].at + Vec3::new(0.0, 0.0, 5.0));
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 6})).unwrap();
        h.frames(2);
        let after = kind(&h).clone();
        assert!(after.drag.is_none() && after.moved[i], "moved by hand");
        assert!(after.shown_dots[i].at.dist(to) < 0.3, "the surface runs through where it was let go: {:?} → {:?}", to, after.shown_dots[i].at);
        let far = (0..36usize)
            .filter(|&p| (p % 6).abs_diff(2) > 2 || (p / 6).abs_diff(2) > 2)
            .map(|p| after.shown_dots[p].at.dist(before[p].at))
            .fold(0.0, f64::max);
        assert!(far < 0.05, "the dots beyond its ring stay: {far}");
        assert!(after.fit.is_some_and(|f| f.0 > rms), "farther from the scan now");
        assert_ne!(gpu_shade(&h.app).unwrap().version, cloud.version, "coloured again");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let kept = edit::editable(&h.app.session, Some("FreeForm1"), "test").unwrap();
        let (u, v) = (after.shown_dots[i].u, after.shown_dots[i].v);
        assert!(kept.surface.eval(u, v).unwrap().dist(after.shown_dots[i].at) < 1e-6, "kept as edited");
    }

    /// Add Face in Edit Surface: a click on an open edge, then two on the dome beyond it, add a
    /// face sharing the edge and tangent to the first; OK keeps the Free Form as one body of two
    /// faces, which opens again as a network.
    #[test]
    fn add_face_in_edit_surface_grows_the_free_form() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[16, 20, 0], [28, 20, 0], [28, 40, 0], [16, 40, 0]], "count": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.adding = Some(Adding::default());
        }
        // The edge along x = 28.
        let k = kind(&h).clone();
        let (f, side) =
            *k.joins.open.iter().find(|&&(f, s)| s.at(&k.shown[f], 0.5).is_some_and(|p| (p.x - 28.0).abs() < 1.0)).expect("the edge at x = 28");
        let mid = side.at(&k.shown[f], 0.5).unwrap();
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        assert_eq!(kind(&h).adding.as_ref().and_then(|a| a.side), Some((f, side)), "the edge taken");
        let top = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        // Near the middle, clear of the panels round the view.
        for (x, y) in [(38.0, 20.0), (38.0, 40.0)] {
            h.step(&json!({"click": {"world": [x, y, top(x, y)]}})).unwrap();
            h.frames(2);
        }
        let k = kind(&h).clone();
        assert_eq!(k.shown.len(), 2, "a face added: {:?}", k.note);
        assert_eq!(k.joins.seams.len(), 1, "sharing the edge");
        assert!(network::worst_kink(&k.shown, &k.joins) < 0.5, "tangent to the first");
        assert!(k.adding.as_ref().is_some_and(|a| a.side.is_none()), "ready for the next face");
        // Undo Last Change takes the face away again; added again by the same clicks, it is the same.
        let added = k.faces.clone();
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            undo_last(e);
            assert_eq!((e.shown.len(), e.history.len()), (1, 0), "the face taken away");
        }
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        for (x, y) in [(38.0, 20.0), (38.0, 40.0)] {
            h.step(&json!({"click": {"world": [x, y, top(x, y)]}})).unwrap();
            h.frames(2);
        }
        assert_eq!(kind(&h).faces, added, "added again as it was");
        // Delete Face: a click inside the new face takes it away; Undo Last Change brings it back.
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.adding = None;
            e.deleting = true;
        }
        let inner = top(34.0, 30.0);
        h.step(&json!({"click": {"world": [34.0, 30.0, inner]}})).unwrap();
        h.frames(2);
        assert_eq!(kind(&h).faces.len(), 1, "the face clicked taken away: {:?}", kind(&h).note);
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            undo_last(e);
            assert_eq!(e.faces, added, "back again");
            e.deleting = false;
            // Smooth keeps it a network on the scan.
            let goals = network::feet(&e.faces, &loaded(e).unwrap().covered.as_ref().unwrap().points, 2.0, &[true, true]);
            let smooth = network::smoothed(&e.faces, &e.joins, &goals, 0.5).unwrap();
            assert_eq!(smooth.len(), 2);
        }
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let kept = edit::editable(&h.app.session, Some("FreeForm1"), "test").unwrap();
        assert_eq!(kept.faces.len(), 2, "one body of two faces");
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert_eq!(kind(&h).shown.len(), 2, "opens again as a network");
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
