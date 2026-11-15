//! Fit Surface (`scan.fit_surface`): click the scan where the surface goes; the area round the
//! click is marked on the scan and the surface fitted to it drawn over the picture as lines across
//! it, with how close it comes, as the radius, the detail and the smoothing change. The detail is
//! automatic by default: as many control points as bring the surface within half the Scan
//! tolerance (the scan's own, shared with the other scan tools). OK keeps it, trimmed to the
//! area's outline unless Trim to the area is unticked.
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
//! - Lines: a click on a line of the grid chooses the points at its ends, a double-click the
//!   whole line across the faces to the edge, and a drag on a line moves it like a point.
//! - Guides: X, Y and Z through the middle of the points chosen, in the origin axes' colours; a
//!   drag on one moves the choice along that axis only.
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

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::network_edit;
use solidcraft_engine::cmd::scan::surface::network::{self, Dot, Joins, Side};
use solidcraft_engine::cmd::scan::surface::symmetric::{self, MirrorPlane};
use solidcraft_engine::cmd::scan::surface::{self as edit, Covered, EditableSurface, Line};
use solidcraft_engine::geom::bspline::NurbsSurface;
use solidcraft_engine::geom::prelude::*;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit::{self, choice_row};

mod guides;
mod select;
mod shape;

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

/// The smoothing brush's circle to start with, and its least and most, in points.
const BRUSH_SIZE: f32 = 60.0;
const BRUSH_SIZES: std::ops::RangeInclusive<f32> = 8.0..=300.0;

/// The most scan points a drag's preview refits to: the full set (as the command uses) once it
/// is let go.
const PREVIEW_POINTS: usize = 2_500;

/// Each line of a face's grid is sampled at this many points.
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

/// How many tolerances out the deviation colours are deepest.
pub(crate) const DEEPEST: f64 = 8.0;

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
    /// What the last edit did, when it is worth saying (a weld): gone with the next edit.
    told: Option<&'static str>,
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
    /// Every handle chosen (the one `chosen` among them): a drag of one moves them all.
    picked: BTreeSet<usize>,
    /// A box being drawn to choose the handles in it.
    marquee: Option<Box<select::Marquee>>,
    /// What Constrain last held the points to, and the refinements Suggest Refinement offers.
    shape: Box<shape::State>,
    /// How far Move takes the chosen point along the way the surface faces, in mm.
    pub along: f64,
    /// The points within this of one moved go with it, less the further they are (mm; 0, none).
    pub falloff: f64,
    /// What a dragged handle does: [`DRAG_MOVE`] or [`DRAG_SLIDE`].
    pub drag_mode: usize,
    /// How far Slide takes the chosen point (or its line) along u and along v, in mm.
    pub slide: [f64; 2],
    /// A tolerance tried here instead of the scan's own, mm (0: the scan's, see `scan.tolerance`).
    pub tolerance: f64,
    /// The scan's tolerance as changed here, kept by OK.
    keep_tolerance: Option<(String, f64)>,
    /// The scan under the surface is coloured by how far it is from it (on by default); off, the
    /// scan is drawn as it is, so nothing sits under the points but the scan itself.
    pub colours: bool,
    /// Surface Check on each face shown.
    check: Vec<edit::SurfaceCheck>,
    /// The drag going on (boxed: the dialog's values stay small).
    drag: Option<Box<Drag>>,
    /// Add Face, while it is on: the open edge taken and the far corners clicked.
    adding: Option<Adding>,
    /// Delete Face is on: a click on a face takes it away.
    deleting: bool,
    /// Crease Edge (`Some(true)`) or Remove Crease (`Some(false)`) is on: a click on a shared edge
    /// creases it, or makes it smooth again.
    creasing: Option<bool>,
    /// The Smoothing Brush is on: a drag evens out the points under its circle, and a stroke is
    /// going (one step of Undo Last Change for the whole stroke).
    brushing: bool,
    stroke: bool,
    /// The last crease click: the edge, and how many edits the history had after it (a
    /// double-click on it takes it back and creases its whole chain instead).
    last_crease: Option<(usize, usize)>,
    /// The brush's circle on the screen, in points, and how hard it smooths, 0 to 1.
    pub brush_size: f32,
    pub brush_strength: f64,
    /// The shared edges creased by hand, and those made smooth by hand, each by its ends: the
    /// faces are held tangent across them or not as these say, whatever angle they meet at now.
    creases: Vec<(Vec3, Vec3)>,
    smooths: Vec<(Vec3, Vec3)>,
    /// Symmetry: the plane chosen (0 none; then YZ, XZ, XY and the design's construction planes),
    /// the pairs of points while the Free Form is a mirror image in it, and what choosing it did.
    pub symmetry: usize,
    mirror: Option<Box<symmetric::Pairs>>,
    mirror_note: Option<Box<Result<String, String>>>,
    /// The plane the design keeps the Free Form a mirror image in, offered among the planes when
    /// it is none of them.
    kept_plane: Option<Box<MirrorPlane>>,
    /// Symmetry turned off: the plane whose other half Keep One Half takes away, and whether a
    /// click on a face now chooses the half to keep.
    half_offer: Option<Box<MirrorPlane>>,
    keeping_half: bool,
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
    creases: Vec<(Vec3, Vec3)>,
    smooths: Vec<(Vec3, Vec3)>,
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
            told: None,
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
            picked: BTreeSet::new(),
            marquee: None,
            shape: Box::default(),
            along: 1.0,
            falloff: 0.0,
            drag_mode: DRAG_MOVE,
            slide: [1.0, 0.0],
            tolerance: 0.0,
            keep_tolerance: None,
            colours: true,
            check: Vec::new(),
            drag: None,
            adding: None,
            deleting: false,
            creasing: None,
            brushing: false,
            stroke: false,
            last_crease: None,
            brush_size: BRUSH_SIZE,
            brush_strength: 0.5,
            creases: Vec::new(),
            smooths: Vec::new(),
            symmetry: 0,
            mirror: None,
            mirror_note: None,
            kept_plane: None,
            half_offer: None,
            keeping_half: false,
            shade: None,
            restudy: false,
            history: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct Loaded {
    target: EditableSurface,
    /// What the design keeps of it besides its faces: edges creased or held smooth by hand, its
    /// symmetry plane.
    kept: network_edit::Kept,
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
    /// The scan the points are of, and its own tolerance as the design keeps it, mm.
    scan: String,
    base: f64,
    shade: network::Shade,
    values: Vec<f32>,
    version: u64,
    dots: Shaded,
    /// The tolerance and colours the dots were coloured with.
    key: (u64, crate::scan_deviation::Colours),
    /// Where the scan's points were when it was looked for ([`edit::scan_print`]): moved since,
    /// it is looked for again.
    print: Option<u64>,
    /// The network's points when the scan under them was looked for, and how far from the faces
    /// it was looked: faces moved well away from there have it looked for again.
    studied: Vec<Vec3>,
    reach: f64,
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
    /// Where every dot (or network point) was when the drag began: a soft selection moves those
    /// near the points dragged from there.
    start: Arc<Vec<Vec3>>,
    /// Alt-dragging a dot on an open edge: the open edges it is on (a face grows from the one the
    /// drag leaves most squarely), and where the pointer has taken it.
    grow: Vec<(usize, Side)>,
    to: Option<Vec3>,
    /// A corner on the network's edge dragged over another such corner: that corner, which it is
    /// welded to when let go.
    weld: Option<usize>,
    /// Dragging along a guide: the axis (0 X, 1 Y, 2 Z) and where along it the press was, from
    /// `from`.
    axis: Option<(usize, f64)>,
    /// Taken by a line of the grid, not a handle: `from` is the place on the line pressed, and
    /// no one point goes to the pointer.
    on_line: bool,
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

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    match id {
        "scan.edit_surface" => {
            // The preference `edit_surface_on_surface` off opens on the control net with snapping
            // off, as Edit Surface did before its points moved onto the surface.
            let k = if app.preferences.edit_surface_on_surface {
                EditSurface::default()
            } else {
                EditSurface { handles: HANDLES_NET, snap: SNAP_OFF, ..EditSurface::default() }
            };
            Some((Kind::ScanEditSurface(k), Vec::new()))
        }
        "scan.fit_surface" => {
            // The radius the scan's size gives, shown from the start (the selected or only scan's).
            let radius = solidcraft_engine::cmd::scan::default_surface_radius(&app.session, None).unwrap_or(0.0);
            Some((
                Kind::ScanFitSurface(FitSurface { radius, ..FitSurface::default() }),
                vec![crate::selection::SelInput::new("Point", crate::selection::FACES, false)],
            ))
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
    /// [`AREA_ROUND`] (within `radius` of the point) or [`AREA_FACE`] (the whole face round it).
    pub area: usize,
    /// The radius of the area round the point, in mm; 0 until the scan's own default is known.
    pub radius: f64,
    /// [`DETAIL_AUTO`] (control points chosen for the tolerance) or [`DETAIL_SET`] (`controls`).
    pub detail: usize,
    pub controls: usize,
    pub smoothing: f64,
    /// The tolerance changed here, mm (0: the scan's), the one in force, and a change OK keeps.
    pub tolerance: f64,
    within: f64,
    keep_tolerance: Option<(String, f64)>,
    /// OK trims the surface to the area's outline.
    pub trim: bool,
    shown: Option<FitShown>,
}

/// Fit Surface's area: within the radius of the point, or the whole face round it (until the
/// surface turns away).
pub const AREA_ROUND: usize = 0;
pub const AREA_FACE: usize = 1;

/// Fit Surface's detail: chosen for the tolerance, or the control points set.
pub const DETAIL_AUTO: usize = 0;
pub const DETAIL_SET: usize = 1;

impl Default for FitSurface {
    fn default() -> Self {
        FitSurface {
            area: AREA_ROUND,
            radius: 0.0,
            detail: DETAIL_AUTO,
            controls: 8,
            smoothing: 0.1,
            tolerance: 0.0,
            within: 0.0,
            keep_tolerance: None,
            trim: true,
            shown: None,
        }
    }
}

impl FitSurface {
    /// The control points asked for: none when they are chosen for the tolerance.
    fn asked(&self) -> Option<usize> {
        (self.detail == DETAIL_SET).then_some(self.controls)
    }

    /// The reach the area is grown to: the radius, or none for the whole face.
    fn reach(&self) -> f64 {
        if self.area == AREA_FACE { f64::INFINITY } else { self.radius }
    }
}

#[derive(Clone, Debug)]
struct FitShown {
    /// The point, the radius, the control points asked for, the smoothing, the tolerance and the
    /// design's revision fitted for.
    key: ([u64; 3], u64, Option<usize>, u64, u64, u64),
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
    // Before the Tolerance row has drawn: the scan's own.
    if !(k.within > 0.0) {
        k.within = crate::scan_fidelity::effective(app, Some(&body), k.tolerance);
    }
    let key = (
        [point.x.to_bits(), point.y.to_bits(), point.z.to_bits()],
        k.reach().to_bits(),
        k.asked(),
        k.smoothing.to_bits(),
        k.within.to_bits(),
        app.session.revision,
    );
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = solidcraft_engine::cmd::scan::surface_preview(&app.session, Some(&body), point, k.reach(), k.asked(), k.smoothing, k.within)
        .map_err(|e| e.to_string())
        .map(|found| {
            let rgb = solidcraft_theme::Tokens::get().analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
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
    let areas = [tr("ui.scan_fit_area_round"), tr("ui.scan_fit_area_face")];
    choice_row(ui, tr("ui.scan_fit_area"), "scan_fit_area", &mut k.area, &areas);
    if k.area == AREA_ROUND {
        form_row(ui, tr("ui.scan_fit_radius"), |ui| ui_kit::length_field(ui, &mut k.radius, &units, decimals));
        k.radius = k.radius.max(0.0);
    }
    let scan = clicked(inputs).map(|(body, _)| body);
    let t = crate::scan_fidelity::row(app, ui, scan.as_deref(), &mut k.tolerance);
    (k.within, k.keep_tolerance) = (t.mm, t.keep);
    let details = [tr("ui.scan_fit_detail_auto"), tr("ui.scan_fit_detail_set")];
    choice_row(ui, tr("ui.scan_fit_detail"), "scan_fit_detail", &mut k.detail, &details);
    if k.detail == DETAIL_SET {
        form_row(ui, tr("ui.scan_fit_controls"), |ui| ui_kit::number_field(ui, &mut k.controls, 4..=30, 0.2, ""));
    } else if let Some(Ok((found, _, _))) = k.shown.as_ref().map(|s| &s.result) {
        let chosen = found.controls.to_string();
        form_row(ui, tr("ui.scan_fit_controls"), |ui| ui_kit::text(ui, &chosen, ui_kit::Text::Cell));
    }
    let format = |v: f64| format!("{v:.2}");
    form_row(ui, tr("ui.scan_fit_smoothing"), |ui| {
        ui_kit::slider(ui, &mut k.smoothing, 0.0..=1.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((found, _, _))) => {
            let note = solidcraft_ui_text::format(
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
    form_row(ui, tr("ui.scan_fit_trim"), |ui| crate::dialogs::check(ui, &mut k.trim));
    ui_kit::text(ui, tr("ui.scan_fit_hint"), ui_kit::Text::Hint("scan_fit_hint"));
    false
}

/// What OK runs: the surface fitted round the point clicked.
pub fn fit_commands(k: &FitSurface, inputs: &[crate::selection::SelInput]) -> Result<Vec<(String, Value)>, String> {
    let (body, point) = clicked(inputs).ok_or_else(|| tr("ui.scan_fit_hint").to_owned())?;
    k.shown.as_ref().ok_or("still fitting")?.result.as_ref().map_err(Clone::clone)?;
    let mut p = json!({
        "body": body,
        "point": [point.x, point.y, point.z],
        "smoothing": k.smoothing,
        "tolerance": format!("{} mm", k.within),
        "trim": k.trim,
    });
    if let Some(n) = k.asked() {
        p["controls"] = json!(n);
    }
    if k.area == AREA_FACE {
        p["whole_face"] = json!(true);
    } else {
        p["radius"] = json!(format!("{} mm", k.radius));
    }
    let mut out: Vec<(String, Value)> = crate::scan_fidelity::keep(k.keep_tolerance.as_ref()).into_iter().collect();
    out.push(("scan.fit_surface".into(), p));
    Ok(out)
}

/// The area fitted to, marked on the scan while the dialog is open.
pub(crate) fn gpu_fit_marks(app: &SolveApp) -> Option<solidcraft_viewport_gpu::view::GpuCloud> {
    let Some(Kind::ScanFitSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let (_, version, dots) = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(solidcraft_viewport_gpu::view::GpuCloud {
        id: FIT_CLOUD_ID,
        version: *version,
        dots: dots.clone(),
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
        style: solidcraft_viewport_gpu::view::CloudStyle {
            mode: solidcraft_viewport_gpu::view::ColorMode::Own,
            range: [0.0, 1.0],
            filters: Default::default(),
            point_size: 3.0,
        },
    })
}

/// The Deviation colours row: whether the scan under the surface is coloured by how far it is.
pub(crate) fn colours_row(ui: &mut egui::Ui, on: &mut bool) {
    form_row(ui, tr("ui.scan_deviation_colours"), |ui| {
        let response = ui_kit::checkbox(ui, "", on);
        crate::scenario::publish_target("dialog:scan_deviation_colours", response.rect);
    });
}

/// How much wider than its core a mark's dark casing is (both sides together), in points.
const CASING: f32 = 2.0;

/// The dark casing under every mark a Free Form dialog draws over the picture: the theme's marker
/// halo, so a line or a point reads over the scan, its colouring and the selection colour alike.
pub(crate) fn casing() -> egui::Color32 {
    solidcraft_theme::Tokens::get().icons.marker_halo
}

/// A line over the picture, on its casing.
pub(crate) fn cased_line(painter: &egui::Painter, points: Vec<egui::Pos2>, stroke: egui::Stroke) {
    painter.add(egui::Shape::line(points.clone(), egui::Stroke::new(stroke.width + CASING, casing())));
    painter.add(egui::Shape::line(points, stroke));
}

/// A segment over the picture, on its casing.
pub(crate) fn cased_segment(painter: &egui::Painter, ends: [egui::Pos2; 2], stroke: egui::Stroke) {
    painter.line_segment(ends, egui::Stroke::new(stroke.width + CASING, casing()));
    painter.line_segment(ends, stroke);
}

/// A filled point over the picture, ringed by its casing.
pub(crate) fn cased_dot(painter: &egui::Painter, at: egui::Pos2, radius: f32, fill: egui::Color32) {
    painter.circle_filled(at, radius + CASING / 2.0 + 0.5, casing());
    painter.circle_filled(at, radius, fill);
}

/// Where the grid's lines run on `surface`: the `u` of each column of its points and the `v` of
/// each row, one for each control point's place on it ([`network::dot_places`]). The dots a user
/// drags are at these places, so each dot is where a line each way crosses: the lines and the
/// points are one grid.
pub(crate) fn grid_params(surface: &NurbsSurface) -> (Vec<f64>, Vec<f64>) {
    let [nu, nv] = surface.count();
    let places = network::dot_places(surface);
    let us = (0..nu).filter_map(|i| places.get(i).map(|p| p.0)).collect();
    let vs = (0..nv).filter_map(|j| places.get(j * nu).map(|p| p.1)).collect();
    (us, vs)
}

/// The grid over `surface` over the picture: a line along each row and each column of its points,
/// through the places of the dots. With `edges` the first and last of each way are drawn too (a
/// preview with nothing else drawing its sides); without, only the lines inside, as the sides are
/// drawn on their own.
pub(crate) fn draw_grid(painter: &egui::Painter, proj: &crate::viewport::Proj, surface: &NurbsSurface, stroke: egui::Stroke, edges: bool) {
    let [[u0, u1], [v0, v1]] = surface.domain();
    let at = |j: usize, a: f64, b: f64| a + (b - a) * j as f64 / SAMPLES as f64;
    let (us, vs) = grid_params(surface);
    let inside = |list: &[f64]| -> Vec<f64> {
        let (skip, take) = if edges { (0, list.len()) } else { (1, list.len().saturating_sub(2)) };
        list.iter().copied().skip(skip).take(take).collect()
    };
    for (along_u, fixed) in [(true, inside(&vs)), (false, inside(&us))] {
        for f in fixed {
            let curve: Option<Vec<egui::Pos2>> = (0..=SAMPLES)
                .map(|j| {
                    let (u, v) = if along_u { (at(j, u0, u1), f) } else { (f, at(j, v0, v1)) };
                    surface.eval(u, v).ok().and_then(|p| proj.to_screen(p))
                })
                .collect();
            if let Some(curve) = curve {
                cased_line(painter, curve, stroke);
            }
        }
    }
}

/// A circle of `radius` round `centre`, square to `normal`, over the picture.
fn draw_reach(painter: &egui::Painter, proj: &crate::viewport::Proj, centre: Vec3, normal: Vec3, radius: f64, stroke: egui::Stroke) {
    let Some(x) = normal.any_perp().normalized() else { return };
    let y = normal.cross(x);
    let ring: Option<Vec<egui::Pos2>> = (0..=64)
        .map(|j| {
            let a = std::f64::consts::TAU * f64::from(j) / 64.0;
            proj.to_screen(centre + (x * a.cos() + y * a.sin()) * radius)
        })
        .collect();
    if let Some(ring) = ring {
        cased_line(painter, ring, stroke);
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
    // The design changed: the scan moved (or was edited) is looked for under the faces again.
    if let Some(sh) = k.shade.as_ref()
        && edit::scan_print(&app.session, &sh.scan) != sh.print
    {
        k.restudy = true;
    }
    let cmd = "scan.edit_surface";
    let found = edit::editable(&app.session, k.target.as_deref(), cmd).map_err(|e| e.to_string());
    if let (Some(Ok(l)), Ok(target)) = (&mut k.loaded, &found)
        && l.target.feature == target.feature
        && l.target.faces == target.faces
    {
        l.revision = revision;
        if k.restudy {
            study(app, k);
        }
        return;
    }
    let loaded = found.map(|target| {
        let kept = network_edit::kept(&app.session, &target);
        Box::new(Loaded { target, kept, covered: None, sample: Arc::new(Vec::new()), revision })
    });
    let faces = loaded.as_ref().ok().map(|l| l.target.faces.clone());
    k.loaded = Some(loaded);
    if let Some(faces) = faces {
        reset_to(k, faces);
        apply_kept(app, k);
        study(app, k);
    }
}

/// What the design keeps of the Free Form put back after the dialog started again from its faces:
/// its edges creased or held smooth by hand, and Symmetry on in its plane (while it is still a
/// mirror image in it).
fn apply_kept(app: &SolveApp, k: &mut EditSurface) {
    let Some(kept) = loaded(k).map(|l| l.kept.clone()) else { return };
    k.creases = kept.creases;
    k.smooths = kept.smooths;
    k.joins = Arc::new(joins_of(k, &k.faces));
    k.net = network::network_points(&k.faces, &k.joins);
    k.kept_plane = kept.symmetry.map(Box::new);
    if let Some(plane) = kept.symmetry {
        refresh_mirror(k, Some(plane));
        k.mirror_note = None;
        if k.mirror.is_some() {
            k.symmetry = symmetry_planes(app, k).iter().position(|(_, p)| same_plane(p, &plane)).map_or(0, |i| i + 1);
        }
    }
    let faces = k.faces.clone();
    show_faces(k, faces, false, None);
}

/// Whether two mirror planes are one: facing the same way (or the other) through the same place.
fn same_plane(a: &MirrorPlane, b: &MirrorPlane) -> bool {
    a.normal.dot(b.normal).abs() > 1.0 - 1e-9 && a.side(b.point).abs() < 1e-6
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
    k.shade = found.map(|(scan, shade)| {
        let print = edit::scan_print(&app.session, &scan);
        Box::new(Shading {
            base: crate::scan_fidelity::of(app, Some(&scan)),
            scan,
            shade,
            values: Vec::new(),
            version: 0,
            dots: Shaded::default(),
            key: (0, [[0; 3]; 5]),
            print,
            studied: k.shown_net.clone(),
            reach: k.faces.iter().map(edit::default_reach).fold(1.0, f64::max),
        })
    });
    let shown = k.shown.clone();
    show_faces(k, shown, false, None);
}

/// Whether the faces shown have gone well away from where the scan under them was looked for
/// (more than a quarter of the distance it was looked within), so scan they now lie over is not
/// coloured, and scan they have left still is.
fn outgrown(k: &EditSurface) -> bool {
    let Some(sh) = k.shade.as_ref() else { return false };
    sh.studied.len() != k.shown_net.len() || sh.studied.iter().zip(&k.shown_net).any(|(a, b)| a.dist(*b) > sh.reach * 0.25)
}

/// Whether the seam with ends `a` and `b` is the edge `e` (either way round).
fn same_edge(e: &(Vec3, Vec3), a: Vec3, b: Vec3) -> bool {
    (e.0.dist(a) < 1e-6 && e.1.dist(b) < 1e-6) || (e.0.dist(b) < 1e-6 && e.1.dist(a) < 1e-6)
}

/// How `faces` join, with the seams creased or made smooth by hand held so (found again by their
/// ends, so a face added or the points doubled keeps them).
fn joins_of(k: &EditSurface, faces: &[NurbsSurface]) -> Joins {
    let mut joins = network::joins(faces).unwrap_or_default();
    for i in 0..joins.seams.len() {
        let Some((a, b)) = network::seam_ends(faces, &joins, i) else { continue };
        if k.creases.iter().any(|e| same_edge(e, a, b)) {
            joins = network::without_tangency(&joins, i);
        } else if k.smooths.iter().any(|e| same_edge(e, a, b)) {
            joins = network::with_tangency(faces, &joins, i);
        }
    }
    joins
}

/// The dialog started again from `faces`: nothing moved, nothing chosen, no crease but the faces' own.
fn reset_to(k: &mut EditSurface, faces: Vec<NurbsSurface>) {
    k.creases.clear();
    k.smooths.clear();
    k.creasing = None;
    k.last_crease = None;
    k.mirror = None;
    k.mirror_note = None;
    k.symmetry = 0;
    k.half_offer = None;
    k.keeping_half = false;
    k.joins = Arc::new(joins_of(k, &faces));
    k.net = network::network_points(&faces, &k.joins);
    k.moved = vec![false; k.net.len()];
    select::clear(k);
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
    k.told = None;
    k.shape.note = None;
    k.shape.suggestions.clear();
    if k.history.len() >= HISTORY {
        k.history.remove(0);
    }
    k.history.push(Before {
        faces: k.faces.clone(),
        moved: k.moved.clone(),
        reshaped: k.reshaped,
        creases: k.creases.clone(),
        smooths: k.smooths.clone(),
    });
}

/// The last edit undone: the faces as they were before it.
fn undo_last(k: &mut EditSurface) {
    let Some(before) = k.history.pop() else { return };
    let restudy = before.faces.len() != k.faces.len();
    k.creases = before.creases;
    k.smooths = before.smooths;
    k.joins = Arc::new(joins_of(k, &before.faces));
    k.net = network::network_points(&before.faces, &k.joins);
    k.moved = before.moved;
    k.reshaped = before.reshaped;
    select::clear(k);
    k.line = None;
    k.drag = None;
    k.faces = before.faces.clone();
    k.restudy |= restudy;
    let plane = k.mirror.take().map(|p| p.plane);
    show_faces(k, before.faces, false, None);
    refresh_mirror(k, plane);
}

/// The faces' shape changed (a face added, points doubled): `faces` as edited from now on, what
/// was moved kept moved where it still is.
fn reshaped_to(k: &mut EditSurface, faces: Vec<NurbsSurface>) {
    remember(k);
    let moved_at: Vec<Vec3> = k.net.iter().zip(&k.moved).filter(|(_, m)| **m).map(|(p, _)| *p).collect();
    k.joins = Arc::new(joins_of(k, &faces));
    k.net = network::network_points(&faces, &k.joins);
    k.moved = k.net.iter().map(|p| moved_at.iter().any(|q| q.dist(*p) < 1e-9)).collect();
    k.reshaped = true;
    select::clear(k);
    k.faces = faces.clone();
    k.restudy = true;
    let plane = k.mirror.take().map(|p| p.plane);
    show_faces(k, faces, false, None);
    refresh_mirror(k, plane);
}

/// How far from the symmetry plane an edge may be to be put onto it: a tenth of the net's size.
fn mirror_gap(k: &EditSurface) -> f64 {
    k.faces.iter().map(edit::default_reach).fold(1.0, f64::max)
}

/// The pairs found again in `plane` after the faces changed shape (a face added, points doubled,
/// an edit undone), the Free Form made an exact mirror image; Symmetry turned off, and said so,
/// when it is one no longer.
fn refresh_mirror(k: &mut EditSurface, plane: Option<MirrorPlane>) {
    let Some(plane) = plane else { return };
    let tolerance = symmetric::pairing_tolerance(&k.faces, mirror_gap(k));
    match symmetric::pairs_of(&k.faces, &k.joins, plane, tolerance) {
        Ok(pairs) => {
            k.mirror = Some(Box::new(pairs));
            let faces = k.faces.clone();
            show_faces(k, faces, false, None);
            k.faces = k.shown.clone();
            k.net = k.shown_net.clone();
        }
        Err(_) => {
            k.mirror = None;
            k.symmetry = 0;
            k.mirror_note = Some(Box::new(Err(tr("ui.scan_edit_symmetry_lost").to_owned())));
        }
    }
}

/// The planes Symmetry offers: the origin planes, then the design's construction planes, then the
/// plane the design keeps the Free Form a mirror image in, when it is none of those.
fn symmetry_planes(app: &SolveApp, k: &EditSurface) -> Vec<(String, MirrorPlane)> {
    let mut planes: Vec<(String, MirrorPlane)> = [("YZ", Vec3::X), ("XZ", Vec3::Y), ("XY", Vec3::Z)]
        .into_iter()
        .filter_map(|(n, normal)| MirrorPlane::new(Vec3::ZERO, normal).map(|p| (n.to_owned(), p)))
        .collect();
    for (_, name, pl) in solidcraft_engine::view::construction_planes(&app.session) {
        if let Some(p) = MirrorPlane::new(pl.origin, pl.x.cross(pl.y)) {
            planes.push((name, p));
        }
    }
    if let Some(kept) = k.kept_plane.as_deref().copied().filter(|kept| !planes.iter().any(|(_, p)| same_plane(p, kept))) {
        planes.push((tr("ui.scan_edit_symmetry_kept").to_owned(), kept));
    }
    planes
}

/// Symmetry chosen (`None` for none): a half mirrored and joined on the plane (one step of Undo
/// Last Change), a Free Form that is a mirror image already paired; else refused, and said why.
fn apply_symmetry(k: &mut EditSurface, plane: Option<(String, MirrorPlane)>) {
    k.keeping_half = false;
    let Some((name, plane)) = plane else {
        // Turned off: both halves stay, and Keep One Half is offered to take one side away.
        k.half_offer = k.mirror.take().filter(|_| is_network(k)).map(|p| Box::new(p.plane));
        k.mirror_note = k.half_offer.as_ref().map(|_| Box::new(Ok(tr("ui.scan_edit_symmetry_off").to_owned())));
        return;
    };
    k.half_offer = None;
    match symmetric::mirrored(&k.faces, plane, mirror_gap(k)) {
        Ok((faces, pairs, how)) => match how {
            symmetric::How::Already => {
                k.mirror = None;
                refresh_mirror(k, Some(pairs.plane));
                k.mirror_note = Some(Box::new(Ok(solidcraft_ui_text::format("ui.scan_edit_symmetry_already", &[("plane", &name)]))));
            }
            symmetric::How::Mirrored { added, onto } => {
                k.mirror = None;
                reshaped_to(k, faces);
                refresh_mirror(k, Some(plane));
                k.mirror_note = Some(Box::new(Ok(solidcraft_ui_text::format(
                    "ui.scan_edit_symmetry_mirrored",
                    &[("plane", &name), ("added", &added.to_string()), ("onto", &onto.to_string())],
                ))));
            }
        },
        Err(e) => {
            k.mirror = None;
            k.symmetry = 0;
            k.mirror_note = Some(Box::new(Err(e.to_string())));
        }
    }
}

/// The Symmetry row: the plane, what choosing it did, and the pairs while it is on.
fn symmetry_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface) {
    let planes = symmetry_planes(app, k);
    let mut labels: Vec<&str> = vec![tr("ui.scan_edit_symmetry_none")];
    labels.extend(planes.iter().map(|(n, _)| n.as_str()));
    let before = k.symmetry.min(labels.len() - 1);
    let mut choice = before;
    choice_row(ui, tr("ui.scan_edit_symmetry"), "scan_edit_symmetry", &mut choice, &labels);
    if choice != before {
        k.symmetry = choice;
        k.adding = None;
        k.deleting = false;
        k.creasing = None;
        select::clear(k);
        apply_symmetry(k, choice.checked_sub(1).and_then(|i| planes.get(i)).map(|(n, p)| (n.clone(), *p)));
    }
    if let Some(pairs) = &k.mirror {
        let line = solidcraft_ui_text::format(
            "ui.scan_edit_symmetry_value",
            &[("pairs", &pairs.pairs().to_string()), ("on", &pairs.on_plane_count().to_string())],
        );
        ui_kit::text(ui, &line, ui_kit::Text::Note("scan_edit_symmetry_value"));
    }
    match k.mirror_note.as_deref() {
        Some(Ok(note)) => {
            ui_kit::text(ui, note, ui_kit::Text::Note("scan_edit_symmetry_note"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_edit_symmetry_note", true));
        }
        None => {}
    }
    if k.mirror.is_some() {
        ui_kit::text(ui, tr("ui.scan_edit_symmetry_hint"), ui_kit::Text::Hint("scan_edit_symmetry_hint"));
    }
    // Symmetry turned off: Keep One Half, then a click on a face of the half to keep.
    if k.half_offer.is_some() {
        let style = if k.keeping_half { ui_kit::Button::SmallSelected } else { ui_kit::Button::Normal };
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_keep_half"), style)).clicked() {
            k.keeping_half = !k.keeping_half;
            k.adding = None;
            k.deleting = false;
            k.creasing = None;
            k.brushing = false;
            select::clear(k);
        }
    }
}

/// Keep One Half's click: the faces on the far side of the plane from the face clicked taken away
/// (one step of Undo Last Change).
fn keep_half_click(k: &mut EditSurface, face: usize) {
    let Some(plane) = k.half_offer.as_deref().copied() else { return };
    let Some(middle) = k.shown.get(face).and_then(|s| {
        let [[u0, u1], [v0, v1]] = s.domain();
        s.eval(0.5 * (u0 + u1), 0.5 * (v0 + v1)).ok()
    }) else {
        return;
    };
    match symmetric::half_kept(&k.faces, plane, middle) {
        Ok(faces) => {
            reshaped_to(k, faces);
            k.half_offer = None;
            k.keeping_half = false;
            k.kept_plane = None;
            k.mirror_note = None;
            k.note = None;
        }
        Err(e) => k.note = Some(e.to_string()),
    }
}

/// `faces` (the dialog's with one added last) given the added face's mirror image too, while
/// Symmetry is on: the faces it grew from refitted on both sides alike.
fn with_twins(k: &EditSurface, faces: Vec<NurbsSurface>) -> Vec<NurbsSurface> {
    match &k.mirror {
        Some(pairs) if !faces.is_empty() => symmetric::with_twin_face_after(&k.faces, &faces, pairs.plane, mirror_gap(k))
            .or_else(|| symmetric::with_twin_face(&faces, pairs.plane, faces.len() - 1, mirror_gap(k)))
            .unwrap_or(faces),
        _ => faces,
    }
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
    k.picked = k.chosen.into_iter().collect();
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
    // With Symmetry on, exactly a mirror image: each pair at the mean of the two.
    let faces = match &k.mirror {
        Some(pairs) => pairs.kept(&faces, &k.joins).unwrap_or(faces),
        None => faces,
    };
    k.shown_net = network::network_points(&faces, &k.joins);
    k.shown_dots = network::dots(&faces, &k.joins);
    // Coarser while a drag goes (it runs every frame), as Surface Check itself once it is let go.
    let grid = if dragging { CHECK_GRID } else { edit::SURFACE_CHECK_GRID };
    k.check = faces.iter().filter_map(|f| edit::check_surface(f, grid)).collect();
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
        sh.key = (0, [[0; 3]; 5]);
    }
    k.shown = faces;
    // Let go well away from where the scan was looked for: it is looked for again.
    if !dragging && outgrown(k) {
        k.restudy = true;
    }
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
    let mirrored;
    let moves = match &k.mirror {
        Some(pairs) => {
            mirrored = pairs.moves(moves);
            mirrored.as_slice()
        }
        None => moves,
    };
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
    let mirrored;
    let moves = match &k.mirror {
        Some(pairs) => {
            mirrored = pairs.moves(moves);
            mirrored.as_slice()
        }
        None => moves,
    };
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

/// The shared edge nearest `at` on the screen, within [`EDGE_GRAB`].
fn seam_at(k: &EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) -> Option<usize> {
    k.joins
        .seams
        .iter()
        .enumerate()
        .filter_map(|(i, seam)| {
            let curve = side_on_screen(proj, k.shown.get(seam.a)?, seam.a_side);
            let d = curve.windows(2).map(|w| distance_to_segment(at, w[0], w[1])).fold(f32::INFINITY, f32::min);
            Some((i, d))
        })
        .filter(|(_, d)| *d <= EDGE_GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// Seam `seam` creased (`crease`), or made smooth again: the faces held tangent across it or not
/// from now on, and the points near it fitted to the scan again, so they take the scan's edge
/// there, or round it over. One step of Undo Last Change.
fn crease_click(k: &mut EditSurface, seam: usize, crease: bool) {
    if crease_seams(k, &[seam], crease) {
        k.last_crease = Some((seam, k.history.len()));
    }
}

/// A double-click on seam `seam`: it and the seams it runs on into through the grid's corners
/// creased (or made smooth) at once. The single click just before it, on the same seam, is taken
/// back first, so the whole chain is one step of Undo Last Change.
fn crease_chain_click(k: &mut EditSurface, seam: usize, crease: bool) {
    let ends = network::seam_ends(&k.faces, &k.joins, seam);
    if k.last_crease == Some((seam, k.history.len())) {
        undo_last(k);
    }
    k.last_crease = None;
    // Found again by its ends: the undo may have read the seams afresh.
    let Some(seam) = ends.and_then(|(a, b)| network::seam_between(&k.faces, &k.joins, a, b, 1e-6)) else { return };
    let chain = network::crease_chain(&k.faces, &k.joins, seam);
    crease_seams(k, &chain, crease);
}

/// Seams `seams` creased (`crease`), or made smooth again, each with its mirror image while
/// Symmetry is on: one step of Undo Last Change. False when every one was so already (said so).
fn crease_seams(k: &mut EditSurface, seams: &[usize], crease: bool) -> bool {
    let state = network::creased(&k.joins);
    let mut picked: Vec<(usize, Vec3, Vec3)> = Vec::new();
    for &seam in seams {
        let Some((a, b)) = network::seam_ends(&k.faces, &k.joins, seam) else { continue };
        if state.get(seam).copied() == Some(crease) || picked.iter().any(|(i, _, _)| *i == seam) {
            continue;
        }
        picked.push((seam, a, b));
        // With Symmetry on, the seam's mirror image too.
        if let Some(pairs) = &k.mirror {
            let (ra, rb) = (pairs.plane.reflect(a), pairs.plane.reflect(b));
            if let Some(t) = network::seam_between(&k.faces, &k.joins, ra, rb, 1e-6).filter(|t| !picked.iter().any(|(i, _, _)| i == t)) {
                picked.push((t, ra, rb));
            }
        }
    }
    if picked.is_empty() {
        k.note = Some(tr(if crease { "ui.scan_edit_crease_already" } else { "ui.scan_edit_uncrease_already" }).to_owned());
        return false;
    }
    let (mut creases, mut smooths) = (k.creases.clone(), k.smooths.clone());
    let mut joins = (*k.joins).clone();
    for &(i, p, q) in &picked {
        creases.retain(|e| !same_edge(e, p, q));
        smooths.retain(|e| !same_edge(e, p, q));
        if crease {
            creases.push((p, q))
        } else {
            smooths.push((p, q))
        }
        joins = if crease { network::without_tangency(&joins, i) } else { network::with_tangency(&k.faces, &joins, i) };
    }
    let scan = loaded(k).and_then(|l| l.covered.as_ref()).map(|c| c.points.clone()).unwrap_or_default();
    let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
    let mut faces = k.faces.clone();
    for &(i, _, _) in &picked {
        if let Some(next) = network::refit_near_seam(&faces, &joins, i, &scan, reach, &k.moved) {
            faces = next;
        }
    }
    k.joins = Arc::new(joins);
    show_faces(k, faces, false, None);
    // Remembered with the creases as they were, so Undo Last Change puts the tangency back too.
    commit(k, &[]);
    k.creases = creases;
    k.smooths = smooths;
    k.note = None;
    true
}

/// One step of the smoothing brush at `at` on the screen: the dots within its circle evened out,
/// more toward its middle, held on the scan when Snap is on. The faces shown are kept as they
/// come (the stroke was remembered as it began).
fn brush_at(k: &mut EditSurface, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let size = k.brush_size.max(1.0);
    let under: Vec<(usize, f64)> = k
        .shown_dots
        .iter()
        .enumerate()
        .filter_map(|(i, d)| {
            let dist = proj.to_screen(d.at)?.distance(at);
            (dist <= size).then(|| (i, f64::from(1.0 - dist / size)))
        })
        .collect();
    if under.is_empty() {
        return;
    }
    let ids: Vec<usize> = under.iter().map(|u| u.0).collect();
    let touched = touched_by(k, &ids);
    let goals = match loaded(k).and_then(|l| l.covered.as_ref().map(|_| l.sample.clone())) {
        Some(sample) if k.snap != SNAP_OFF && !sample.is_empty() => {
            let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
            network::feet(&k.faces, &sample, reach, &touched)
        }
        _ => Vec::new(),
    };
    if let Some(faces) = network::brushed(&k.faces, &k.joins, &under, &goals, k.brush_strength) {
        show_faces(k, faces, true, Some(&touched));
        k.faces = k.shown.clone();
        k.net = k.shown_net.clone();
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
    solidcraft_ui_text::tr(id)
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
        many => solidcraft_ui_text::format("ui.scan_edit_faces_value", &[("body", &l.target.body), ("faces", &many.len().to_string())]),
    };
    form_row(ui, solidcraft_ui_text::tr_label("Surface"), |ui| ui_kit::text(ui, &name, ui_kit::Text::Cell));
    let odd = k.joins.irregular().count();
    if odd > 0 {
        let line = solidcraft_ui_text::format("ui.scan_edit_irregular_value", &[("count", &odd.to_string())]);
        form_row(ui, tr("ui.scan_edit_irregular"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Report("scan_edit_irregular", false)));
    }
    let sharp = network::creased(&k.joins).iter().filter(|c| **c).count();
    if sharp > 0 {
        form_row(ui, tr("ui.scan_edit_creases"), |ui| ui_kit::text(ui, &sharp.to_string(), ui_kit::Text::Cell));
    }
    let kinds = [tr("ui.scan_edit_handles_dots"), tr("ui.scan_edit_handles_net")];
    let before = k.handles;
    choice_row(ui, tr("ui.scan_edit_handles"), "scan_edit_handles", &mut k.handles, &kinds);
    if k.handles != before {
        select::clear(k);
        k.line = None;
        k.adding = None;
        k.creasing = None;
        k.brushing = false;
    }
    let dots = k.handles != HANDLES_NET;
    symmetry_rows(app, ui, k);
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
        let modes = [tr("ui.scan_edit_drag_move"), tr("ui.scan_edit_drag_slide")];
        choice_row(ui, tr("ui.scan_edit_drag"), "scan_edit_drag", &mut k.drag_mode, &modes);
    }
    form_row(ui, tr("ui.scan_edit_falloff"), |ui| ui_kit::length_field(ui, &mut k.falloff, &units, decimals));
    k.falloff = if k.falloff.is_finite() { k.falloff.max(0.0) } else { 0.0 };
    select::rows(ui, k);
    let moved = k.moved.iter().filter(|m| **m).count().to_string();
    form_row(ui, tr("ui.scan_edit_moved"), |ui| ui_kit::text(ui, &moved, ui_kit::Text::Cell));
    // How far the scan is from the surface, and its colouring against the tolerance.
    if let Some((rms, max)) = k.fit.filter(|_| k.shade.is_none()) {
        let fit = solidcraft_ui_text::format(
            "ui.scan_edit_distance_value",
            &[("rms", &crate::prefs::show_mm(app, rms, 1)), ("max", &crate::prefs::show_mm(app, max, 1))],
        );
        ui_kit::text(ui, &format!("{}: {fit}", tr("ui.scan_edit_distance")), ui_kit::Text::Note("scan_edit_distance"));
    }
    if k.shade.is_some() {
        colours_row(ui, &mut k.colours);
        let scan = k.shade.as_ref().map(|sh| sh.scan.clone());
        if let (Some(sh), Some(name)) = (k.shade.as_mut(), scan.as_deref()) {
            sh.base = crate::scan_fidelity::of(app, Some(name));
        }
        let t = crate::scan_fidelity::row(app, ui, scan.as_deref(), &mut k.tolerance);
        k.keep_tolerance = t.keep;
        recolour(k);
        if let Some(stats) = fidelity(app, k).map(|(s, _)| s) {
            crate::scan_fidelity::readout(app, ui, "scan_edit_fidelity", &stats, t.mm, None);
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
            ui_kit::text(ui, tr("ui.scan_edit_folds_hint"), ui_kit::Text::Hint("scan_edit_folds_hint"));
        }
    }
    let edited = k.moved.iter().any(|m| *m) || k.reshaped || k.faces != l.target.faces || kept_changed(k, &l);
    let can_undo = !k.history.is_empty();
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_undo"), ui_kit::Button::Enabled(can_undo))).clicked() && can_undo {
        undo_last(k);
    }
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_reset"), ui_kit::Button::Enabled(edited))).clicked() {
        reset_to(k, l.target.faces.clone());
        apply_kept(app, k);
        k.restudy = true;
    }
    whole_tools(app, ui, k, &l);
    shape::rows(app, ui, k, l.covered.is_some());
    if !dots && !is_network(k) {
        let count = base(k).map_or([0, 0], NurbsSurface::count);
        net_tools(ui, k, count);
    }
    chosen_rows(app, ui, k, &units, decimals);
    if let Some(note) = &k.note {
        ui_kit::text(ui, note, ui_kit::Text::Report("scan_edit_note", true));
    }
    if let Some(told) = k.told {
        ui_kit::text(ui, told, ui_kit::Text::Note("scan_edit_told"));
    }
    let hint = match (&k.adding, dots) {
        _ if k.keeping_half => tr("ui.scan_edit_keep_half_hint"),
        _ if k.deleting => tr("ui.scan_edit_delete_face_hint"),
        _ if k.brushing => tr("ui.scan_edit_brush_hint"),
        _ if k.creasing == Some(true) => tr("ui.scan_edit_crease_hint"),
        _ if k.creasing == Some(false) => tr("ui.scan_edit_uncrease_hint"),
        (Some(a), _) if a.side.is_none() => tr("ui.scan_edit_add_face_edge"),
        (Some(_), _) => tr("ui.scan_edit_add_face_corners"),
        (None, true) => tr("ui.scan_edit_hint_dots"),
        (None, false) => tr("ui.scan_edit_hint"),
    };
    ui_kit::text(ui, hint, ui_kit::Text::Hint("scan_edit_hint"));
    ui_kit::text(ui, tr("ui.scan_edit_select_hint"), ui_kit::Text::Hint("scan_edit_select_hint"));
    // The Smoothing Brush on Ctrl+Shift (Preferences › Tools), with the points on the surface.
    if dots && app.preferences.edit_surface_brush_keys && !k.brushing && k.adding.is_none() && !k.deleting && k.creasing.is_none() {
        ui_kit::text(ui, tr("ui.scan_edit_brush_keys_hint"), ui_kit::Text::Hint("scan_edit_brush_keys_hint"));
    }
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
        k.creasing = None;
        k.brushing = false;
        select::clear(k);
        k.line = None;
    }
    if is_network(k) || k.deleting {
        let style = if k.deleting { ui_kit::Button::SmallSelected } else { ui_kit::Button::Normal };
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_delete_face"), style)).clicked() {
            k.deleting = !k.deleting;
            k.adding = None;
            k.creasing = None;
            k.brushing = false;
            select::clear(k);
        }
    }
    // Crease Edge and Remove Crease: a click on a shared edge then makes it sharp, or smooth.
    if is_network(k) {
        for (mode, id) in [(true, "ui.scan_edit_crease_edge"), (false, "ui.scan_edit_uncrease_edge")] {
            let on = k.creasing == Some(mode);
            let style = if on { ui_kit::Button::SmallSelected } else { ui_kit::Button::Normal };
            if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr(id), style)).clicked() {
                k.creasing = if on { None } else { Some(mode) };
                k.adding = None;
                k.deleting = false;
                k.brushing = false;
                select::clear(k);
                k.line = None;
            }
        }
    }
    // The Smoothing Brush, on points on the surface: its size on the screen and its strength.
    if k.handles == HANDLES_DOTS {
        let style = if k.brushing { ui_kit::Button::SmallSelected } else { ui_kit::Button::Normal };
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_brush"), style)).clicked() {
            k.brushing = !k.brushing;
            k.stroke = false;
            k.adding = None;
            k.deleting = false;
            k.creasing = None;
            select::clear(k);
            k.line = None;
        }
        if k.brushing {
            form_row(ui, tr("ui.scan_edit_brush_size"), |ui| ui_kit::number_field(ui, &mut k.brush_size, BRUSH_SIZES, 2.0, " px"));
            k.brush_size = if k.brush_size.is_finite() { k.brush_size.clamp(*BRUSH_SIZES.start(), *BRUSH_SIZES.end()) } else { BRUSH_SIZE };
            let mut percent = 100.0 * k.brush_strength;
            let format = |v: f64| format!("{v:.0} %");
            let changed = form_row(ui, tr("ui.scan_edit_brush_strength"), |ui| {
                ui_kit::slider(ui, &mut percent, 5.0..=100.0, ui_kit::Slider { format, ..ui_kit::Slider::default() })
            })
            .changed();
            if changed && percent.is_finite() {
                k.brush_strength = (percent / 100.0).clamp(0.05, 1.0);
            }
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
        let some = !k.picked.is_empty();
        let (snap_id, fit_id) =
            if some { ("ui.scan_edit_snap_selected", "ui.scan_edit_fit_selected") } else { ("ui.scan_edit_snap_all", "ui.scan_edit_fit_all") };
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr(snap_id), ui_kit::Button::Normal)).clicked() {
            outcome = Some(snap_all(app, k));
        }
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr(fit_id), ui_kit::Button::Normal)).clicked() {
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
    let wanted = select::scope(k);
    let moves: Vec<(usize, Vec3)> = k
        .shown_dots
        .iter()
        .enumerate()
        .filter(|(i, _)| wanted.binary_search(i).is_ok())
        .filter_map(|(i, d)| on.nearest(d.at).filter(|(_, dist)| *dist <= reach).map(|(q, _)| (i, q)))
        .collect();
    if moves.is_empty() {
        return Err(tr("ui.scan_edit_no_scan").to_owned());
    }
    let faces = network::dots_moved(&k.faces, &k.joins, &moves, 0, true, &[]).ok_or_else(|| tr("ui.scan_edit_nothing").to_owned())?;
    show_faces(k, faces, false, None);
    let all: Vec<usize> = moves.iter().map(|m| m.0).collect();
    commit(k, &all);
    Ok(())
}

/// Every face fitted to the scan under it, the faces held tangent where they met smoothly, the
/// network's own edge (its open sides), the corners where faces meet unevenly and the points moved
/// by hand held where they are; the points moving smoothly with each other, so that the net does not
/// slide over the scan into folds. A fit that would fold a surface that did not is tried again
/// held smoother, and refused if it still does.
fn fit_all(k: &mut EditSurface) -> Result<(), String> {
    let points = loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.points.clone())).ok_or_else(|| tr("ui.scan_edit_no_scan").to_owned())?;
    let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
    let mut held: Vec<bool> = vec![false; k.net.len()];
    for c in k.joins.corners.iter().filter(|c| c.irregular() || (c.border && c.faces == 1)) {
        held[c.point] = true;
    }
    for &(f, side) in &k.joins.open {
        for p in k.joins.side(&k.faces, f, side) {
            held[p] = true;
        }
    }
    // With points chosen only those are fitted (even ones moved by hand); else every point not held
    // or moved by hand.
    let chosen = select::chosen_ids(k);
    let free: Vec<bool> = (0..k.net.len())
        .map(|p| if chosen.is_empty() { !held[p] && !k.moved.get(p).copied().unwrap_or(false) } else { !held[p] && chosen.binary_search(&p).is_ok() })
        .collect();
    let all = vec![true; k.faces.len()];
    let trouble = |faces: &[NurbsSurface]| -> usize {
        faces.iter().filter_map(|f| edit::check_surface(f, edit::SURFACE_CHECK_GRID)).map(|c| c.folds + c.pinches + c.crossings).sum()
    };
    let before = trouble(&k.faces);
    for even in [0.1, 1.0, 10.0] {
        let mut faces = k.faces.clone();
        for _ in 0..3 {
            let goals = network::feet(&faces, &points, reach, &all);
            let moves = network::Moves { free: &free, goals: &goals, tangent: 40.0, even, bend: 1e-4, also_smooth: &[] };
            match network::moved(&faces, &k.joins, &moves) {
                Some(next) => faces = next,
                None => break,
            }
        }
        if trouble(&faces) <= before {
            show_faces(k, faces, false, None);
            commit(k, &[]);
            return Ok(());
        }
    }
    Err(tr("ui.scan_edit_fit_folds").to_owned())
}

/// The point chosen (clicked or last dragged): where it goes, typed, or a distance along the way
/// the surface faces; with the control net of one face, a slide along it.
fn chosen_rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface, units: &str, decimals: u8) {
    let _ = app;
    let dots = k.handles == HANDLES_DOTS;
    let group = select::chosen_ids(k);
    if group.len() > 1 {
        group_rows(ui, k, &group, units, decimals);
        return;
    }
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

/// The way the surface faces at handle `i`.
fn normal_of(k: &EditSurface, i: usize) -> Option<Vec3> {
    if k.handles == HANDLES_DOTS {
        k.shown_dots.get(i).and_then(|d| network::dot_normal(&k.shown, d))
    } else {
        k.joins.at.get(i).and_then(|a| a.first()).and_then(|&(f, at)| k.shown.get(f).and_then(|s| edit::normal_at(s, at)))
    }
}

/// Several points chosen: where their middle goes, typed (they all move by the same amount), and
/// a distance along the way the surface faces at each.
fn group_rows(ui: &mut egui::Ui, k: &mut EditSurface, group: &[usize], units: &str, decimals: u8) {
    let Some(c) = select::centre(k) else { return };
    let line = solidcraft_ui_text::format(
        "ui.scan_edit_selected_value",
        &[("count", &group.len().to_string()), ("all", &select::handle_count(k).to_string())],
    );
    form_row(ui, tr("ui.scan_edit_point"), |ui| ui_kit::text(ui, &line, ui_kit::Text::Cell));
    let mut xyz = [c.x, c.y, c.z];
    let mut typed = false;
    for (axis, v) in ["X", "Y", "Z"].into_iter().zip(&mut xyz) {
        form_row(ui, axis, |ui| {
            let r = ui_kit::length_field(ui, v, units, decimals);
            typed |= r.changed();
            r
        });
    }
    if typed && xyz.iter().all(|v| v.is_finite()) {
        let delta = Vec3::new(xyz[0], xyz[1], xyz[2]) - c;
        let moves: Vec<(usize, Vec3)> = group.iter().filter_map(|&i| select::handle_place(k, i).map(|p| (i, p + delta))).collect();
        move_many(k, &moves);
    }
    form_row(ui, tr("ui.scan_edit_along"), |ui| ui_kit::length_field(ui, &mut k.along, units, decimals));
    if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_move"), ui_kit::Button::Normal)).clicked() && k.along.is_finite() {
        let moves: Vec<(usize, Vec3)> = group.iter().filter_map(|&i| Some((i, select::handle_place(k, i)? + normal_of(k, i)? * k.along))).collect();
        move_many(k, &moves);
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
        // Two buttons to a row: four in one ran past the card's edge, Crease out of reach.
        ui_kit::row(ui, tr("ui.scan_edit_line"), |ui| {
            ui.horizontal(|ui| {
                if ui_kit::button(ui, tr("ui.scan_edit_split"), ui_kit::Button::Small).clicked() {
                    outcome = Some(reshape(k, |s| edit::split_line(s, line).map(|(m, l)| (m, Some(l)))));
                }
                if ui_kit::button(ui, tr("ui.scan_edit_copy"), ui_kit::Button::Small).clicked() {
                    outcome = Some(reshape(k, |s| edit::duplicate_line(s, line, true).map(|(m, l)| (m, Some(l)))));
                }
            })
            .response
        });
        ui_kit::row(ui, "", |ui| {
            ui.horizontal(|ui| {
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
    move_many(k, &[(index, to)]);
}

/// Points (or dots) `moves` put where they go together, as a drag let go there would, those
/// near them going with them as the Falloff says: kept as one step.
fn move_many(k: &mut EditSurface, moves: &[(usize, Vec3)]) {
    let by_hand: Vec<usize> = moves.iter().map(|m| m.0).collect();
    let soft = k.handles == HANDLES_DOTS || is_network(k);
    let moves: Vec<(usize, Vec3)> = if soft && k.falloff > 0.0 {
        let (start, held): (Vec<Vec3>, Vec<bool>) =
            if k.handles == HANDLES_DOTS { (k.shown_dots.iter().map(|d| d.at).collect(), Vec::new()) } else { (k.net.clone(), k.moved.clone()) };
        let from: Vec<(usize, Vec3)> = moves.iter().filter_map(|m| start.get(m.0).map(|p| (m.0, *p))).collect();
        select::soft_moves(&start, &from, moves, k.falloff, &held)
    } else {
        moves.to_vec()
    };
    let moves = moves.as_slice();
    let made = if k.handles == HANDLES_DOTS {
        dot_preview(k, moves)
    } else if base(k).is_some() {
        preview(k, moves).map(|s| vec![s])
    } else {
        net_moved(k, moves)
    };
    if let Some(faces) = made {
        show_faces(k, faces, false, None);
        commit(k, &by_hand);
    }
}

/// The plane Symmetry keeps the Free Form a mirror image in as the dialog has it now.
fn symmetry_now(k: &EditSurface) -> Option<MirrorPlane> {
    k.mirror.as_ref().map(|p| p.plane)
}

/// Whether what the design keeps of the Free Form besides its faces (its edges creased or held
/// smooth by hand, its symmetry plane) would change with OK.
fn kept_changed(k: &EditSurface, l: &Loaded) -> bool {
    if k.faces.len() < 2 && l.kept == network_edit::Kept::default() {
        return false;
    }
    let (creases, smooths) = network::held_by_hand(&k.faces, &k.joins);
    let tolerance = network::ends_tolerance(&k.faces);
    let same_edges =
        |a: &[(Vec3, Vec3)], b: &[(Vec3, Vec3)]| a.len() == b.len() && a.iter().all(|e| b.iter().any(|f| same_edge_within(e, f.0, f.1, tolerance)));
    let same_symmetry = match (symmetry_now(k), l.kept.symmetry) {
        (None, None) => true,
        (Some(a), Some(b)) => same_plane(&a, &b),
        _ => false,
    };
    !(same_edges(&creases, &l.kept.creases) && same_edges(&smooths, &l.kept.smooths) && same_symmetry)
}

/// Whether the edge with ends `a` and `b` is `e` (either way round), each end within `tolerance`.
fn same_edge_within(e: &(Vec3, Vec3), a: Vec3, b: Vec3, tolerance: f64) -> bool {
    (e.0.dist(a) <= tolerance && e.1.dist(b) <= tolerance) || (e.0.dist(b) <= tolerance && e.1.dist(a) <= tolerance)
}

/// What OK runs: the net as edited; every face, once a face was added or a net's shape changed,
/// or for a network, with the edges creased or held smooth by hand and the symmetry plane for the
/// design to keep.
pub fn commands(k: &EditSurface) -> Result<Vec<(String, Value)>, String> {
    let mut all = Vec::new();
    all.extend(crate::scan_fidelity::keep(k.keep_tolerance.as_ref()));
    all.extend(surface_commands(k)?);
    Ok(all)
}

fn surface_commands(k: &EditSurface) -> Result<Vec<(String, Value)>, String> {
    let l = k.loaded.as_ref().ok_or("still loading the surface")?.as_ref().map_err(Clone::clone)?;
    let kept = kept_changed(k, l);
    if k.faces == l.target.faces && !k.reshaped && !kept {
        return Err(solidcraft_ui_text::tr("ui.scan_edit_nothing").into());
    }
    let net_of = |s: &NurbsSurface| {
        let points: Vec<[f64; 3]> = edit::control_points(s).iter().map(|p| [p.x, p.y, p.z]).collect();
        json!({"count": s.count(), "knots": [s.knots()[0].clone(), s.knots()[1].clone()], "degree": s.degree(), "points": points})
    };
    // A surface that was a Free Form of several faces goes back as faces, so what the design keeps
    // of it (its creases, its symmetry plane) goes with them, even down to one face.
    match (k.faces.as_slice(), k.reshaped, l.target.is_network() || l.kept != network_edit::Kept::default()) {
        ([one], false, false) => {
            let points: Vec<[f64; 3]> = edit::control_points(one).iter().map(|p| [p.x, p.y, p.z]).collect();
            Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "points": points}))])
        }
        ([one], true, false) => Ok(vec![("scan.edit_surface".into(), json!({"surface": l.target.body, "net": net_of(one)}))]),
        (many, _, _) => {
            let (creases, smooths) = network::held_by_hand(many, &k.joins);
            let ends = |list: Vec<(Vec3, Vec3)>| list.into_iter().map(|(a, b)| [[a.x, a.y, a.z], [b.x, b.y, b.z]]).collect::<Vec<_>>();
            let symmetry = match symmetry_now(k) {
                Some(p) => json!({"point": [p.point.x, p.point.y, p.point.z], "normal": [p.normal.x, p.normal.y, p.normal.z]}),
                None => json!("none"),
            };
            Ok(vec![(
                "scan.edit_surface".into(),
                json!({
                    "surface": l.target.body,
                    "faces": many.iter().map(net_of).collect::<Vec<_>>(),
                    "kept_edges": {"creases": ends(creases), "smooths": ends(smooths)},
                    "symmetry": symmetry,
                }),
            )])
        }
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
    if !matches!(f.kind, solidcraft_engine::doc::FeatureKind::ScanSurface { .. } | solidcraft_engine::doc::FeatureKind::Import { .. }) {
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

/// The feature that keeps `body` when it is a surface Edit Surface can edit (a Free Form, a fitted
/// surface), for a double-click or a menu to open the editor on exactly that surface.
pub(crate) fn editable_feature(app: &SolveApp, body: &str) -> Option<u64> {
    edit::editable(&app.session, Some(body), "scan.edit_surface").ok().map(|t| t.feature)
}

/// Double-click on a surface in the view: Edit Surface opens on that surface. False when it is not
/// one that can be edited.
pub(crate) fn edit_body(app: &mut SolveApp, body: &str) -> bool {
    editable_feature(app, body).is_some_and(|f| edit_from_history(app, f))
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

/// Whether Ctrl+Shift is held for the Smoothing Brush (Preferences › Tools): `mods` exactly Ctrl
/// (Command on macOS) and Shift, with the preference on.
pub(crate) fn brush_keys_held(app: &SolveApp, mods: egui::Modifiers) -> bool {
    app.preferences.edit_surface_brush_keys && (mods.ctrl || mods.command) && mods.shift && !mods.alt
}

/// The wheel turned with Ctrl+Shift held over the view while Edit Surface shows its points on the
/// surface: the brush's circle grows or shrinks, and the view does not zoom. Before the camera
/// reads the wheel.
pub(crate) fn brush_wheel(app: &mut SolveApp, ctx: &egui::Context, rect: egui::Rect) {
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
    if !over || !app.preferences.edit_surface_brush_keys {
        return;
    }
    let Some(k) = dialog(app).filter(|k| k.handles == HANDLES_DOTS && loaded(k).is_some()) else { return };
    // Each wheel event by the keys held as it was turned.
    let line = ctx.options(|o| o.input_options.line_scroll_speed);
    let (turned, points) = ctx.input(|i| {
        let mut turned = false;
        let mut points = 0.0f32;
        for e in &i.events {
            let egui::Event::MouseWheel { unit, delta, modifiers, phase, .. } = e else { continue };
            if !((modifiers.ctrl || modifiers.command) && modifiers.shift && !modifiers.alt) {
                continue;
            }
            turned = true;
            if *phase == egui::TouchPhase::Move {
                points += match unit {
                    egui::MouseWheelUnit::Point => delta.x + delta.y,
                    egui::MouseWheelUnit::Line => (delta.x + delta.y) * line,
                    egui::MouseWheelUnit::Page => (delta.x + delta.y) * 400.0,
                };
            }
        }
        (turned, points)
    });
    if !turned {
        return;
    }
    if points != 0.0 && points.is_finite() {
        k.brush_size = (k.brush_size * (0.0025 * points).exp()).clamp(*BRUSH_SIZES.start(), *BRUSH_SIZES.end());
    }
    crate::navigation::claim_wheel(ctx);
}

/// The wheel turned with Shift held over the view while Edit Surface has points chosen: the soft
/// selection's radius (Falloff) grows or shrinks, and the view does not zoom. Before the camera
/// reads the wheel.
pub(crate) fn falloff_wheel(app: &mut SolveApp, ctx: &egui::Context, rect: egui::Rect) {
    let over = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| rect.contains(p));
    if !over {
        return;
    }
    let Some(k) = dialog(app).filter(|k| loaded(k).is_some() && (!k.picked.is_empty() || k.drag.is_some())) else { return };
    let line = ctx.options(|o| o.input_options.line_scroll_speed);
    let (turned, points) = ctx.input(|i| {
        let mut turned = false;
        let mut points = 0.0f32;
        for e in &i.events {
            let egui::Event::MouseWheel { unit, delta, modifiers, phase, .. } = e else { continue };
            if !modifiers.shift || modifiers.ctrl || modifiers.command || modifiers.alt {
                continue;
            }
            turned = true;
            if *phase == egui::TouchPhase::Move {
                points += match unit {
                    egui::MouseWheelUnit::Point => delta.x + delta.y,
                    egui::MouseWheelUnit::Line => (delta.x + delta.y) * line,
                    egui::MouseWheelUnit::Page => (delta.x + delta.y) * 400.0,
                };
            }
        }
        (turned, points)
    });
    if !turned {
        return;
    }
    if points != 0.0 && points.is_finite() {
        k.falloff = turned_falloff(k, f64::from(points));
    }
    crate::navigation::claim_wheel(ctx);
}

/// The Falloff radius after the wheel turned `points`: a tenth of the surface's size to begin
/// with, then a quarter more or less for each ~100 points, never below nothing.
fn turned_falloff(k: &EditSurface, points: f64) -> f64 {
    let size = k.shown_net.first().map_or(0.0, |first| {
        let (lo, hi) = k.shown_net.iter().fold((*first, *first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        (hi - lo).len()
    });
    let now = if k.falloff > 0.0 { k.falloff } else { 0.02 * size };
    let next = now * (0.0025 * points).exp();
    // Wheeled right down it is off.
    if next.is_finite() && next >= 0.01 * size.max(1.0) { next } else { 0.0 }
}

/// Whether a handle is being dragged: the drag neither selects nor turns the view.
pub(crate) fn holds_pointer(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanEditSurface(k)) if k.drag.is_some() || k.stroke || k.marquee.is_some())
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
pub(crate) fn choose(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, mods: egui::Modifiers) {
    if dialog(app).is_some_and(|k| k.adding.is_some()) {
        add_face_click(app, proj, at);
        return;
    }
    if let Some(k) = dialog(app).filter(|k| k.keeping_half) {
        if let Some(face) = face_at(k, proj, at) {
            keep_half_click(k, face);
        }
        return;
    }
    if let Some(k) = dialog(app).filter(|k| k.creasing.is_some()) {
        if let (Some(seam), Some(crease)) = (seam_at(k, proj, at), k.creasing) {
            crease_click(k, seam, crease);
        }
        return;
    }
    // A click with the brush on is a dab: one step, one change.
    if let Some(k) = dialog(app).filter(|k| k.brushing && !k.stroke) {
        remember(k);
        brush_at(k, proj, at);
        let shown = k.shown.clone();
        show_faces(k, shown, false, None);
        return;
    }
    if let Some(k) = dialog(app).filter(|k| k.deleting) {
        if let Some(face) = face_at(k, proj, at) {
            // With Symmetry on, the face's mirror image goes with it (the higher one first, so
            // the other's place stays).
            let twin = k.mirror.as_ref().and_then(|p| symmetric::twin_face(&k.faces, p.plane, face, 1e-6)).filter(|t| *t != face);
            let mut gone = vec![face];
            gone.extend(twin);
            gone.sort_unstable();
            let taken = gone.iter().rev().try_fold(k.faces.clone(), |faces, &f| network::without_face(&faces, f));
            match taken {
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
    let Some(k) = dialog(app) else { return };
    let how = select::How::of(mods);
    if let Some(index) = handle_at(k, proj, at) {
        select::pick(k, index, how);
    } else if guides::guide_at(k, proj, at).is_some() {
        // A click on a guide leaves the choice as it is (a drag on it moves the choice).
    } else if let Some((line, _)) = select::line_at(k, proj, at) {
        // A click on a line of the grid chooses the points at its ends.
        select::pick_many(k, &line.ends, how);
    } else if !how.keeps() {
        // A click on nothing chooses nothing, unless keys held keep the choice.
        select::clear(k);
    }
}

/// A double-click at `at`: with Crease Edge or Remove Crease on, the shared edge under it and the
/// edges it runs on into through the grid's corners, at once; else the line of the grid under it
/// and the whole line it is part of (QuickSurface's); else the face under it, all its points
/// chosen.
pub(crate) fn double_click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, mods: egui::Modifiers) {
    if let Some(k) = dialog(app).filter(|k| k.creasing.is_some()) {
        if let (Some(seam), Some(crease)) = (seam_at(k, proj, at), k.creasing) {
            crease_chain_click(k, seam, crease);
        }
        return;
    }
    let Some(k) = dialog(app).filter(|k| k.adding.is_none() && !k.deleting && !k.keeping_half && !k.brushing) else { return };
    let how = select::How::of(mods);
    if guides::guide_at(k, proj, at).is_some() {
        return;
    }
    // On a line of the grid (off the handles), the whole line it is part of, across the faces.
    if handle_at(k, proj, at).is_none()
        && let Some((line, _)) = select::line_at(k, proj, at)
    {
        let whole = select::whole_line(k, line.ends[0], line.ends[1]);
        select::pick_many(k, &whole, how);
        return;
    }
    // Otherwise a double-click on a face chooses all its points.
    if let Some(face) = face_at(k, proj, at) {
        select::pick_face(k, face, how);
    }
}

/// Add Face's click: the open edge under it, then a far corner (a point of the surface near it is
/// that point; else the scan under it, else level with the edge in the plane of the screen). The
/// second corner adds the face.
fn add_face_click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    // The scan under the click, through the surface or a model in front of it.
    let scan = dialog(app).and_then(|k| loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.scan.clone())));
    let on_scan = scan.as_deref().and_then(|scan| crate::scan_snap::scan_hit(app, proj, at, Some(scan))).map(|(_, p)| p);
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
    // A point of the surface near the click is welded to.
    let near_point = k
        .shown_net
        .iter()
        .filter_map(|p| proj.to_screen(*p).map(|s| (*p, s.distance(at))))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p);
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
            let faces = with_twins(k, faces);
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
pub(crate) fn grab(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2, mods: egui::Modifiers, brush_keys: bool) -> bool {
    let copy = mods.alt;
    let Some(k) = dialog(app) else { return false };
    if k.adding.is_some() || k.deleting || k.creasing.is_some() || k.keeping_half {
        return false;
    }
    // A stroke of the brush (on, or Ctrl+Shift held): remembered once, then a step at each place
    // the pointer goes.
    if (k.brushing || brush_keys) && k.handles == HANDLES_DOTS {
        remember(k);
        k.stroke = true;
        k.faces = k.shown.clone();
        k.net = k.shown_net.clone();
        brush_at(k, proj, at);
        return true;
    }
    let how = select::How::of(mods);
    let Some(mut index) = handle_at(k, proj, at) else {
        // A drag begun on a guide moves the choice along its axis.
        if !how.keeps()
            && let Some(axis) = guides::guide_at(k, proj, at)
            && let Some(c) = select::centre(k)
            && let Some(t0) = crate::gizmo::along(proj, at, c, guides::DIRS[axis])
        {
            return begin_choice_drag(k, c, Some((axis, t0)), false);
        }
        // A drag begun on a line of the grid moves it (and the rest of the choice, when both its
        // ends are chosen already).
        if let Some((line, on)) = select::line_at(k, proj, at) {
            if how.keeps() {
                return false;
            }
            let [a, b] = line.ends;
            if !(k.picked.contains(&a) && k.picked.contains(&b)) {
                select::pick_many(k, &[a, b], select::How::Replace);
            }
            return begin_choice_drag(k, on, None, true);
        }
        // A drag begun on nothing draws a box that chooses the handles in it.
        select::begin_box(k, at, how);
        return true;
    };
    // Keys that keep the choice make a press on a handle a click (it is added or taken away).
    if how.keeps() && !copy {
        return false;
    }
    // A drag starts from the faces as shown (a refit the last drag made is kept).
    k.faces = k.shown.clone();
    k.net = k.shown_net.clone();
    if k.handles == HANDLES_DOTS {
        let Some(dot) = k.shown_dots.get(index).copied() else { return false };
        let group = select::dragged_with(k, index);
        let with: Vec<(usize, Vec3)> = group.iter().filter_map(|&i| k.shown_dots.get(i).map(|d| (i, d.at))).collect();
        let start = Arc::new(k.shown_dots.iter().map(|d| d.at).collect::<Vec<_>>());
        // Alt on a dot of an open edge drags a new face out of that edge (QuickSurface's Alt-drag).
        let grow: Vec<(usize, Side)> = if copy {
            k.joins.open.iter().copied().filter(|&(f, side)| k.joins.side(&k.faces, f, side).contains(&index)).collect()
        } else {
            Vec::new()
        };
        k.drag =
            Some(Box::new(Drag { index, from: dot.at, with, slide: None, dot: true, start, grow, to: None, weld: None, axis: None, on_line: false }));
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
    let line = k.line.zip(count).filter(|(l, c)| l.points(*c).contains(&index));
    let group = select::dragged_with(k, index);
    let with: Vec<(usize, Vec3)> = match line {
        Some((l, c)) => l.points(c).into_iter().filter_map(|p| k.net.get(p).map(|q| (p, *q))).collect(),
        None => group.iter().filter_map(|&p| k.net.get(p).map(|q| (p, *q))).collect(),
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
    k.drag = Some(Box::new(Drag {
        index,
        from: k.net[index],
        with,
        slide,
        dot: false,
        start: Arc::new(k.net.clone()),
        grow: Vec::new(),
        to: None,
        weld: None,
        axis: None,
        on_line: false,
    }));
    true
}

/// A drag of every handle chosen, begun off them: along guide `axis` (and where along it the
/// press was), or by a line of the grid; `from` is the place pressed.
fn begin_choice_drag(k: &mut EditSurface, from: Vec3, axis: Option<(usize, f64)>, on_line: bool) -> bool {
    let ids = select::chosen_ids(k);
    let Some(index) = k.chosen.filter(|i| ids.contains(i)).or_else(|| ids.first().copied()) else { return false };
    k.faces = k.shown.clone();
    k.net = k.shown_net.clone();
    let dot = k.handles == HANDLES_DOTS;
    let with: Vec<(usize, Vec3)> = ids.iter().filter_map(|&i| select::handle_place(k, i).map(|p| (i, p))).collect();
    let start = Arc::new(if dot { k.shown_dots.iter().map(|d| d.at).collect() } else { k.net.clone() });
    k.drag = Some(Box::new(Drag { index, from, with, slide: None, dot, start, grow: Vec::new(), to: None, weld: None, axis, on_line }));
    true
}

/// The handle dragged to `to` on the screen.
pub(crate) fn drag_to(app: &mut SolveApp, proj: &crate::viewport::Proj, to: egui::Pos2) {
    if let Some(k) = dialog(app).filter(|k| k.marquee.is_some()) {
        select::drag_box(k, proj, to);
        return;
    }
    if let Some(k) = dialog(app).filter(|k| k.stroke) {
        brush_at(k, proj, to);
        return;
    }
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
    // Along a guide: every point the same way along its axis, nothing snapped.
    let along = drag.axis.and_then(|(axis, t0)| {
        let d = guides::DIRS.get(axis).copied()?;
        crate::gizmo::along(proj, to, drag.from, d).map(|t| d * (t - t0))
    });
    if drag.axis.is_some() && along.is_none() {
        return;
    }
    // In the plane of the screen through where the point was.
    let in_plane = {
        let (o, d) = proj.ray(to);
        let n = proj.cam.back();
        let den = d.dot(n);
        (den.abs() > 1e-12).then(|| o + d * ((drag.from - o).dot(n) / den))
    };
    // Onto the scan under the pointer, when snapping so and there is scan there within reach.
    let under = if snap == SNAP_POINTER
        && along.is_none()
        && let Some(scan) = &scan
    {
        crate::scan_snap::scan_hit(app, proj, to, Some(scan)).map(|(_, point)| point)
    } else {
        None
    };
    let Some(place) = along.map(|d| drag.from + d).or_else(|| under.filter(|p| in_plane.is_none_or(|q| p.dist(q) <= reach)).or(in_plane)) else {
        return;
    };
    let delta = place - drag.from;
    // Each point that goes moved as the one dragged; snapping to the nearest place puts each on the
    // scan's nearest place to where it would go, within reach.
    let nearest = (snap == SNAP_NEAREST && along.is_none())
        .then(|| scan.as_deref().and_then(|b| solidcraft_engine::cmd::scan::curve::projector(&app.session, Some(b), "scan.edit_surface").ok()));
    let moves: Vec<(usize, Vec3)> = drag
        .with
        .iter()
        .map(|&(i, from)| {
            let to = if i == drag.index && under.is_some() && !drag.on_line { place } else { from + delta };
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
    // A corner on the network's edge over another such corner: it goes onto it, and is welded to
    // it when let go.
    let weld = if drag.axis.is_some() || drag.on_line { None } else { weld_target(k, proj, drag.index, to) };
    if let Some(d) = k.drag.as_mut() {
        d.weld = weld;
    }
    let moves: Vec<(usize, Vec3)> = match weld.and_then(|w| k.shown_net.get(w).copied()) {
        Some(onto) => moves.into_iter().map(|(i, p)| if i == drag.index { (i, onto) } else { (i, p) }).collect(),
        None => moves,
    };
    // A soft selection: the points near those dragged go with them, less the further they are. (The
    // control net of one face does it by itself.)
    let moves = if drag.dot || is_network(k) {
        let held: Vec<bool> = if drag.dot { Vec::new() } else { k.moved.clone() };
        select::soft_moves(&drag.start, &drag.with, &moves, k.falloff, &held)
    } else {
        moves
    };
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

/// Whether network point `p` is a corner on the network's edge.
fn edge_corner(k: &EditSurface, p: usize) -> bool {
    k.joins.corners.iter().any(|c| c.point == p && c.border)
}

/// The corner on the network's edge under the pointer at `at` that the dragged point `index` (a
/// corner on the edge too) would be welded to: another one, within [`GRAB`] on the screen.
fn weld_target(k: &EditSurface, proj: &crate::viewport::Proj, index: usize, at: egui::Pos2) -> Option<usize> {
    if !edge_corner(k, index) {
        return None;
    }
    k.joins
        .corners
        .iter()
        .filter(|c| c.border && c.point != index)
        .filter_map(|c| Some((c.point, proj.to_screen(*k.shown_net.get(c.point)?)?.distance(at))))
        .filter(|(_, d)| *d <= GRAB)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(p, _)| p)
}

/// Corner `point` dropped on corner `onto`: one point from then on, and open sides whose ends then
/// meet sewn into a shared edge, held tangent and fitted to the scan near it where the faces go on
/// from each other (else a crease); with Symmetry the mirror image welded too. One step of Undo
/// Last Change.
fn weld(k: &mut EditSurface, point: usize, onto: usize) {
    let (Some(&from), Some(&to)) = (k.net.get(point), k.shown_net.get(onto)) else { return };
    let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
    let Some((mut faces, mut sewn, _, mut smooths)) = network_edit::corner_welded(&k.faces, &k.joins, point, to, reach, &k.creases, &k.smooths)
    else {
        k.note = Some(tr("ui.scan_edit_nothing").to_owned());
        return;
    };
    // With Symmetry on, the mirror image too: the corners nearest the two reflected places.
    if let Some(plane) = k.mirror.as_ref().map(|p| p.plane)
        && plane.reflect(from).dist(from) > 1e-6
        && let Ok(joins) = network::joins_keeping(&faces, &k.creases, &smooths, network::ends_tolerance(&faces))
    {
        let places = network::network_points(&faces, &joins);
        let nearest = |q: Vec3, but: Option<usize>| {
            joins
                .corners
                .iter()
                .filter(|c| c.border && Some(c.point) != but)
                .min_by(|a, b| places[a.point].dist(q).total_cmp(&places[b.point].dist(q)))
                .map(|c| c.point)
        };
        if let Some(p) = nearest(plane.reflect(from), None)
            && let Some(o) = nearest(plane.reflect(to), Some(p))
            && let Some((f, n, _, both)) = network_edit::corner_welded(&faces, &joins, p, places[o], reach, &k.creases, &smooths)
        {
            faces = f;
            sewn += n;
            smooths = both;
        }
    }
    let before = k.smooths.clone();
    k.smooths = smooths;
    reshaped_to(k, faces);
    if let Some(b) = k.history.last_mut() {
        b.smooths = before;
    }
    k.note = None;
    k.told = Some(tr(if sewn > 0 { "ui.scan_edit_sewn" } else { "ui.scan_edit_welded" }));
}

/// The drag let go: the faces as previewed are kept and the point counts as moved by hand; a
/// face dragged out of an open edge is added, out to where the edge was taken; a corner let go
/// on another corner is welded to it.
pub(crate) fn release(app: &mut SolveApp) {
    if let Some(k) = dialog(app).filter(|k| k.marquee.is_some()) {
        select::end_box(k);
        return;
    }
    // A stroke of the brush ends: the faces as brushed are kept, measured again on every point.
    if let Some(k) = dialog(app).filter(|k| k.stroke) {
        k.stroke = false;
        let shown = k.shown.clone();
        show_faces(k, shown, false, None);
        return;
    }
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
                let faces = with_twins(k, faces);
                reshaped_to(k, faces);
                k.note = None;
            }
            Err(e) => k.note = Some(e.to_string()),
        }
        return;
    }
    let Some(k) = dialog(app) else { return };
    let Some(drag) = k.drag.take() else { return };
    if let Some(onto) = drag.weld {
        weld(k, drag.index, onto);
        return;
    }
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
    if !k.colours {
        return None;
    }
    let sh = k.shade.as_ref().filter(|s| s.version != 0)?;
    Some(shade_cloud(SHADE_CLOUD_ID, sh.version, &sh.dots))
}

/// A colouring as a cloud for the GPU viewport, under `id`.
pub(crate) fn shade_cloud(id: u64, version: u64, shaded: &Shaded) -> GpuCloud {
    GpuCloud {
        id,
        version,
        dots: shaded.dots.clone(),
        segments: crate::scan_dots::none(),
        triangles: shaded.triangles.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: SHADE_PIXELS },
    }
}

/// The colouring built again when the values, the tolerance or the theme's colours have changed.
fn recolour(k: &mut EditSurface) {
    let tolerance = tolerance_now(k);
    let Some(sh) = k.shade.as_mut() else { return };
    let key = (tolerance.to_bits(), crate::scan_deviation::colours());
    if sh.key == key && sh.version != 0 {
        return;
    }
    sh.dots = shade_dots(&sh.shade, &sh.values, tolerance, key.1);
    sh.version = next_shade_version();
    sh.key = key;
}

/// The tolerance the colouring uses: the one tried here, else the scan's own.
fn tolerance_now(k: &EditSurface) -> f64 {
    if k.tolerance.is_finite() && k.tolerance > 0.0 { k.tolerance } else { k.shade.as_ref().map_or(0.1, |sh| sh.base) }
}

/// What the colouring of the scan under the faces adds up to, and its tolerance, for the
/// viewport's key.
pub(crate) fn fidelity(_app: &SolveApp, k: &EditSurface) -> Option<(crate::scan_fidelity::Stats, f64)> {
    let sh = k.shade.as_ref()?;
    let tolerance = tolerance_now(k);
    Some((crate::scan_fidelity::stats(&sh.values, tolerance)?, tolerance))
}

/// A new version for a colouring's dots (each is uploaded again).
pub(crate) fn next_shade_version() -> u64 {
    SHADE_VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    })
}

/// A colouring's records for the GPU viewport: the scan's triangles under the surface painted, or
/// its points as dots where too many triangles lie under it ([`network::PAINT_LIMIT`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct Shaded {
    pub dots: Arc<Vec<u8>>,
    pub triangles: Arc<Vec<u8>>,
}

/// The scan under the surface coloured by how far each part of it is from the surface, against
/// `tolerance`: its triangles painted where the shade has them, else its points as dots.
pub(crate) fn shade_dots(shade: &network::Shade, values: &[f32], tolerance: f64, colours: crate::scan_deviation::Colours) -> Shaded {
    let rgb = |v: f32| solidcraft_scan_scale(v, tolerance).map(|s| crate::scan_deviation::colour(colours, s));
    if !shade.paint.is_empty() {
        let mut out = Vec::with_capacity(shade.paint.len() * crate::scan_dots::DOT_SIZE);
        for t in &shade.paint {
            let Some(&v) = values.get(t.point as usize) else { continue };
            let Some(rgb) = rgb(v) else { continue };
            let cell = solidcraft_viewport_gpu::view::Cell { corners: t.corners, value: [v; 3], rgb, ..Default::default() };
            solidcraft_viewport_gpu::view::cloud_cell(&mut out, &cell);
        }
        return Shaded { dots: crate::scan_dots::none(), triangles: Arc::new(out) };
    }
    let mut out = Vec::with_capacity(shade.points.len() * crate::scan_dots::DOT_SIZE);
    for (p, v) in shade.points.iter().zip(values) {
        let Some(rgb) = rgb(*v) else { continue };
        let dot = crate::scan_dots::Dot { at: p.at.to_f32(), value: *v, rgb };
        crate::scan_dots::cloud_dot(&mut out, &dot);
    }
    Shaded { dots: Arc::new(out), triangles: crate::scan_dots::none() }
}

/// Where a distance sits on the colouring's scale, -1 to 1 (0 within the tolerance).
pub(crate) fn solidcraft_scan_scale(v: f32, tolerance: f64) -> Option<f64> {
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

/// A line of the grid on the screen: along the face between its two dots (the surface's line
/// through them), or straight between two points of the control net.
fn line_on_screen(k: &EditSurface, proj: &crate::viewport::Proj, l: &select::GridLine) -> Vec<egui::Pos2> {
    let ends = || l.ends.iter().filter_map(|e| select::handle_place(k, *e)).filter_map(|p| proj.to_screen(p)).collect();
    if k.handles != HANDLES_DOTS {
        return ends();
    }
    let Some(face) = k.shown.get(l.face) else { return ends() };
    let places = network::dot_places(face);
    let (Some(&(u0, v0)), Some(&(u1, v1))) = (places.get(l.at[0]), places.get(l.at[1])) else { return ends() };
    const STEPS: usize = 8;
    (0..=STEPS)
        .filter_map(|n| {
            let t = n as f64 / STEPS as f64;
            face.eval(u0 + (u1 - u0) * t, v0 + (v1 - v0) * t).ok().and_then(|p| proj.to_screen(p))
        })
        .collect()
}

/// The faces shown, their sides, the handles (dots on the surface or the control net), the
/// corners that are not a grid's, and Add Face's edge and guides, over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let t = solidcraft_theme::Tokens::get();
    if let Some(d) = app.dialog.as_ref()
        && let Kind::ScanFitSurface(k) = &d.kind
    {
        if let Some(Ok((found, _, _))) = k.shown.as_ref().map(|s| &s.result) {
            draw_grid(painter, proj, &found.surface, egui::Stroke::new(1.5, t.surface_accent), true);
            // The radius as a circle round the point, square to the way the area faces.
            if let (AREA_ROUND, Some(normal), Some((_, centre))) = (k.area, found.facing, clicked(&d.inputs)) {
                draw_reach(painter, proj, centre, normal, k.radius, egui::Stroke::new(1.5, t.text));
            }
        }
        return;
    }
    let Some(Kind::ScanEditSurface(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    if k.shown.is_empty() {
        return;
    }
    // One grid: with the points on the surface its lines run through them; with the control net
    // shown the net is the grid, and the surface's lines would be a second one over it.
    if k.handles == HANDLES_DOTS {
        for face in &k.shown {
            draw_grid(painter, proj, face, egui::Stroke::new(1.0, t.surface_accent.gamma_multiply(0.8)), false);
        }
    }
    // The sides: shared ones thin, the open edge bold (Add Face's edge under the pointer, or the
    // one taken, in the accent).
    let pointer = painter.ctx().input(|i| i.pointer.hover_pos());
    let hovered = match (&k.adding, pointer) {
        (Some(a), _) if a.side.is_some() => a.side,
        (Some(_), Some(at)) => open_edge_at(k, proj, at),
        _ => None,
    };
    // Shared sides thin; a crease (the faces meet at an angle, not held tangent) in Continuity
    // Analysis's crease colour; with Crease Edge or Remove Crease on, the one under the pointer bold.
    let creased = network::creased(&k.joins);
    let under_pointer = k.creasing.and_then(|_| pointer.and_then(|at| seam_at(k, proj, at)));
    for (i, seam) in k.joins.seams.iter().enumerate() {
        if let Some(s) = k.shown.get(seam.a) {
            let sharp = creased.get(i).copied().unwrap_or(false);
            let (width, colour) = match (under_pointer == Some(i), sharp) {
                (true, _) => (4.0, t.accent),
                (false, true) => (2.5, t.continuity[2]),
                (false, false) => (1.5, t.surface_accent),
            };
            cased_line(painter, side_on_screen(proj, s, seam.a_side), egui::Stroke::new(width, colour));
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
        cased_line(painter, side_on_screen(proj, s, side), egui::Stroke::new(width, colour));
    }
    // Delete Face: the face under the pointer outlined in the error colour.
    if k.deleting
        && let Some(f) = pointer.and_then(|at| face_at(k, proj, at))
        && let Some(s) = k.shown.get(f)
    {
        for side in [Side::FirstRow, Side::LastRow, Side::FirstColumn, Side::LastColumn] {
            cased_line(painter, side_on_screen(proj, s, side), egui::Stroke::new(3.0, t.error));
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
            cased_segment(painter, [n0, *c0], guide);
            if let Some(c1) = rest.first() {
                cased_segment(painter, [n1, *c1], guide);
                cased_segment(painter, [*c0, *c1], guide);
            }
        }
        for c in a.corners.iter().filter_map(|p| proj.to_screen(*p)) {
            cased_dot(painter, c, HANDLE + 1.0, t.warning);
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
            cased_segment(painter, [*e0, *f0], guide);
            cased_segment(painter, [*e1, *f1], guide);
            cased_segment(painter, [*f0, *f1], guide);
        }
    }
    let moved_fill = crate::theme::model_color(t.analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8));
    let fill_of = |i: usize, on_line: bool| {
        if k.chosen == Some(i) || k.picked.contains(&i) {
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
                cased_dot(painter, s, HANDLE, fill_of(i, false));
            }
        }
    } else {
        // The control net, face by face.
        let screen: Vec<Option<egui::Pos2>> = k.shown_net.iter().map(|p| proj.to_screen(*p)).collect();
        let net = egui::Stroke::new(1.0, t.text.gamma_multiply(0.8));
        for (face, ids) in k.shown.iter().zip(&k.joins.of) {
            let [nu, nv] = face.count();
            for j in 0..nv {
                for i in 0..nu {
                    let here = ids.get(j * nu + i).and_then(|p| screen.get(*p).copied().flatten());
                    for next in [(i + 1 < nu).then(|| j * nu + i + 1), (j + 1 < nv).then(|| (j + 1) * nu + i)].into_iter().flatten() {
                        if let (Some(a), Some(b)) = (here, ids.get(next).and_then(|p| screen.get(*p).copied().flatten())) {
                            cased_segment(painter, [a, b], net);
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
                painter.rect_filled(rect.expand(CASING / 2.0 + 0.5), 1.5, casing());
                painter.rect_filled(rect, 1.0, fill_of(i, on_line));
            }
        }
    }
    // The lines of the grid with both ends chosen in the accent; the one under the pointer, which
    // a click chooses, lighter.
    if k.adding.is_none() && !k.deleting && k.creasing.is_none() && !k.brushing && !k.keeping_half {
        let free = k.drag.is_none() && k.marquee.is_none();
        let hover = pointer
            .filter(|p| free && handle_at(k, proj, *p).is_none() && guides::guide_at(k, proj, *p).is_none())
            .and_then(|p| select::line_at(k, proj, p))
            .map(|(l, _)| l);
        let chosen = if k.picked.len() >= 2 {
            select::grid_lines(k).into_iter().filter(|l| l.ends.iter().all(|e| k.picked.contains(e))).collect()
        } else {
            Vec::new()
        };
        for (lines, stroke) in [(chosen, egui::Stroke::new(3.0, t.accent)), (hover.into_iter().collect(), egui::Stroke::new(3.0, t.accent_soft))] {
            for l in lines {
                let on = line_on_screen(k, proj, &l);
                if on.len() >= 2 {
                    cased_line(painter, on, stroke);
                }
            }
        }
    }
    guides::draw(k, painter, proj, pointer);
    // Points chosen ringed; the box being drawn; the soft selection's circle round the choice.
    for &i in &k.picked {
        if let Some(s) = select::handle_place(k, i).and_then(|p| proj.to_screen(p)) {
            painter.circle_stroke(s, HANDLE + 2.5, egui::Stroke::new(1.5, t.accent));
        }
    }
    if let Some(m) = &k.marquee {
        painter.rect_filled(m.rect(), 0.0, t.accent.gamma_multiply(0.15));
        painter.rect_stroke(m.rect(), 0.0, egui::Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    }
    if k.falloff > 0.0
        && let Some(c) = select::centre(k)
        && let Some(mid) = proj.to_screen(c)
    {
        let n = proj.cam.back();
        let side = n.cross(if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y }).normalized().unwrap_or(Vec3::X);
        if let Some(edge) = proj.to_screen(c + side * k.falloff) {
            painter.circle_stroke(mid, mid.distance(edge), egui::Stroke::new(1.0, t.accent_soft));
        }
    }
    // Symmetry: the plane as a sheet over the Free Form, and the points on it ringed.
    if let Some(pairs) = &k.mirror
        && let Some(first) = k.shown_net.first()
    {
        let (lo, hi) = k.shown_net.iter().fold((*first, *first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let grow = (hi - lo).len() * 0.1 + 1.0;
        let pad = Vec3::new(grow, grow, grow);
        let corners: Option<Vec<egui::Pos2>> =
            crate::dialogs_scan::sheet(pairs.plane.point, pairs.plane.normal, lo - pad, hi + pad).iter().map(|p| proj.to_screen(*p)).collect();
        if let Some(corners) = corners.filter(|c| c.len() >= 3) {
            let fill = t.construction_plane.gamma_multiply(0.25);
            painter.extend(crate::viewport::projected_polygon(&corners, fill, egui::Stroke::new(1.0, t.construction_geom)));
        }
        for (i, d) in k.shown_dots.iter().enumerate().filter(|(i, _)| pairs.is_on_plane(*i)) {
            if let Some(s) = proj.to_screen(if k.handles == HANDLES_DOTS { d.at } else { k.shown_net.get(i).copied().unwrap_or(d.at) }) {
                painter.circle_stroke(s, HANDLE + 2.5, egui::Stroke::new(1.5, t.construction_geom));
            }
        }
    }
    // The smoothing brush's circle round the pointer: while it is on, or Ctrl+Shift is held
    // (Preferences › Tools) with the points on the surface.
    let keys = brush_keys_held(app, painter.ctx().input(|i| i.modifiers));
    if (k.brushing || keys || k.stroke)
        && k.handles == HANDLES_DOTS
        && let Some(at) = pointer
        && proj.rect.contains(at)
    {
        painter.circle_stroke(at, k.brush_size, egui::Stroke::new(1.5, t.accent));
    }
    // The corner a dragged corner is welded to when let go.
    if let Some(w) = k.drag.as_ref().and_then(|d| d.weld)
        && let Some(s) = k.shown_net.get(w).and_then(|p| proj.to_screen(*p))
    {
        painter.circle_stroke(s, HANDLE + 4.0, egui::Stroke::new(2.5, t.accent));
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

    /// A ridge scanned as a grid, `n` squares a side over 60 mm: z = 10 − 0.2·|x − 30|, as an STL.
    fn ridge_stl(n: u32) -> String {
        let at = |i: u32, j: u32| {
            let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
            [x, y, 10.0 - 0.2 * (x - 30.0).abs()]
        };
        let mut stl = String::from("solid r\n");
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
        stl + "endsolid r\n"
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
        assert!(cloud.dots.is_empty(), "painted, not dotted");
        assert!(cloud.triangles.len() > 100 * crate::scan_dots::DOT_SIZE, "the scan's triangles under it painted: {}", cloud.triangles.len());
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

    /// Alt-drag a point on an open edge (QuickSurface's Alt-drag): a face is drawn out of that edge
    /// as far as the point was taken, added when it is let go, sharing the edge and tangent to the
    /// face it grows from.
    #[test]
    fn alt_drag_on_an_open_edge_grows_a_face() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[16, 20, 0], [28, 20, 0], [28, 40, 0], [16, 40, 0]], "count": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let k = kind(&h).clone();
        // The dot in the middle of the edge along x = 28.
        let (i, dot) = k
            .shown_dots
            .iter()
            .enumerate()
            .filter(|(_, d)| (d.at.x - 28.0).abs() < 1.0)
            .min_by(|a, b| (a.1.at.y - 30.0).abs().total_cmp(&(b.1.at.y - 30.0).abs()))
            .map(|(i, d)| (i, *d))
            .expect("a dot on the edge");
        let to = dot.at + Vec3::new(10.0, 0.0, 0.0);
        h.step(&json!({"drag": [{"world": [dot.at.x, dot.at.y, dot.at.z]}, {"world": [to.x, to.y, to.z]}], "steps": 6, "alt": true})).unwrap();
        h.frames(2);
        let after = kind(&h).clone();
        assert_eq!(after.shown.len(), 2, "a face grown out of the edge: {:?}", after.note);
        assert_eq!(after.joins.seams.len(), 1, "sharing it");
        assert!(network::worst_kink(&after.shown, &after.joins) < 0.5, "tangent to the first");
        assert!(after.shown_dots.iter().any(|d| (d.at.x - 38.0).abs() < 1.5), "out to where the point was taken");
        assert!(!after.moved.get(i).copied().unwrap_or(false), "the edge's point itself not moved");
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
            k.detail = DETAIL_SET;
            k.controls = 4;
        }
        h.frames(2);
        let (narrow, _) = count(&h).unwrap();
        assert!(narrow < wide, "a smaller radius, a smaller area: {narrow} < {wide}");
        // The whole face takes no radius: the dome as far as it faces the way the click does.
        if let Some(Kind::ScanFitSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.area = AREA_FACE;
        }
        h.frames(2);
        let (face, _) = count(&h).unwrap();
        assert!(face > narrow, "the whole face is more than the small round: {face} > {narrow}");
        if let Some(Kind::ScanFitSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.area = AREA_ROUND;
        }
        h.frames(2);
        assert_ne!(gpu_fit_marks(&h.app).unwrap().version, marks.version, "marked again");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.model.state().body("Surface1").is_some(), "kept");
    }
    /// Edit Surface opens on points on the surface with Snap set to Under the pointer; with the
    /// preference `edit_surface_on_surface` off it opens on the control net with snapping off, as
    /// it did before.
    #[test]
    fn the_preference_keeps_edit_surface_opening_on_the_control_net() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(20)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [30, 30, 8], "radius": 20, "controls": 5})).unwrap();
        assert!(h.app.preferences.edit_surface_on_surface, "on by default");
        h.app.start("scan.edit_surface");
        h.frames(2);
        assert_eq!((kind(&h).handles, kind(&h).snap), (HANDLES_DOTS, SNAP_POINTER), "the new default");
        h.step(&json!({"click": {"handle": "dialog:cancel"}})).unwrap();
        h.settle();
        h.app.preferences.edit_surface_on_surface = false;
        h.app.start("scan.edit_surface");
        h.frames(2);
        assert_eq!((kind(&h).handles, kind(&h).snap), (HANDLES_NET, SNAP_OFF), "the old default, kept behind the preference");
    }

    /// The lines of a face's grid and the dots a user drags are one grid: every dot lies where a
    /// line along u crosses one along v, for a fitted surface, a Free Form face and a face added
    /// to it (a network), and the lines are as many as the points are each way.
    #[test]
    fn the_surface_lines_run_through_the_dots() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[16, 20, 0], [28, 20, 0], [28, 40, 0], [16, 40, 0]], "count": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let through = |k: &EditSurface| {
            assert!(!k.shown.is_empty() && !k.shown_dots.is_empty(), "the surface and its dots shown");
            let grids: Vec<(Vec<f64>, Vec<f64>)> = k.shown.iter().map(grid_params).collect();
            for (face, (us, vs)) in k.shown.iter().zip(&grids) {
                let [nu, nv] = face.count();
                assert_eq!((us.len(), vs.len()), (nu, nv), "a line for each column and each row of points");
            }
            for d in &k.shown_dots {
                let (face, (us, vs)) = (&k.shown[d.face], &grids[d.face]);
                let on_line = us.iter().any(|&u| vs.iter().any(|&v| face.eval(u, v).is_ok_and(|p| p.dist(d.at) < 1e-6)));
                assert!(on_line, "the dot at {:?} on face {} is where two lines cross", d.at, d.face);
            }
        };
        through(kind(&h));
        assert_eq!(kind(&h).shown.len(), 1, "one face to start with");

        // A face added on the open edge at x = 28 (Add Face's clicks): a network of two faces, whose
        // dots (a point on the shared side is one dot) still lie where each face's lines cross.
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.adding = Some(Adding::default());
        }
        let k = kind(&h).clone();
        let (f, side) =
            *k.joins.open.iter().find(|&&(f, s)| s.at(&k.shown[f], 0.5).is_some_and(|p| (p.x - 28.0).abs() < 1.0)).expect("the edge at x = 28");
        let mid = side.at(&k.shown[f], 0.5).unwrap();
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        let top = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        for (x, y) in [(38.0, 20.0), (38.0, 40.0)] {
            h.step(&json!({"click": {"world": [x, y, top(x, y)]}})).unwrap();
            h.frames(2);
        }
        assert_eq!(kind(&h).shown.len(), 2, "a face added: {:?}", kind(&h).note);
        through(kind(&h));
    }

    /// What the highlight paints in the selection colour: whole bodies redrawn (planned, in
    /// blocks, instanced) and loose triangles, counted.
    fn selection_painted(h: &crate::scenario::Harness) -> [usize; 4] {
        let lit = crate::viewport::build_highlight(&h.app);
        let planned = lit.selected_planned.len();
        let blocks = lit.selected_blocks.len();
        let instanced = lit.selected_instanced.len();
        [planned, blocks, instanced, lit.merged().tris.len()]
    }

    /// While a Free Form dialog is open (Edit Surface, Fit Surface after its click), the scan is
    /// not painted in the selection colour, which hid the surface's lines and points; with the
    /// preference `free_form_plain_scan` off it is, as before. The Deviation colours row hides the
    /// scan's colouring by distance and brings it back.
    #[test]
    fn free_form_dialogs_draw_the_scan_without_the_selection_colour() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(20)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.fit_surface", json!({"point": [30, 30, 8], "radius": 20, "controls": 5})).unwrap();
        let scan = h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).expect("the scan");
        h.app.session.selection = vec![solidcraft_engine::Sel::Body { name: scan.clone() }];
        h.frames(1);
        let nothing = [0, 0, 0, 0];
        let selected = selection_painted(&h);
        assert_ne!(selected, nothing, "the scan selected is painted in the selection colour with no dialog open");

        assert!(h.app.preferences.free_form_plain_scan, "on by default");
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert!(crate::dialogs_scan_free_form::plain_scan(&h.app));
        assert_eq!(selection_painted(&h), nothing, "Edit Surface draws the scan plain");

        // The Deviation colours row: the scan's colouring by distance off, then on again.
        assert!(gpu_shade(&h.app).is_some(), "the scan coloured by distance");
        h.step(&json!({"click": {"handle": "dialog:scan_deviation_colours"}})).unwrap();
        h.frames(1);
        assert!(!kind(&h).colours && gpu_shade(&h.app).is_none(), "Deviation colours off");
        h.step(&json!({"click": {"handle": "dialog:scan_deviation_colours"}})).unwrap();
        h.frames(1);
        assert!(kind(&h).colours && gpu_shade(&h.app).is_some(), "Deviation colours on again");

        // The old look, kept behind the preference.
        h.app.preferences.free_form_plain_scan = false;
        h.frames(1);
        assert_ne!(selection_painted(&h), nothing, "the scan painted as selected, as before");
        h.step(&json!({"click": {"handle": "dialog:cancel"}})).unwrap();
        h.settle();

        // Fit Surface, on a scan with no surface over it: the point clicked on the scan is the
        // dialog's input, which the old look paints in the selection colour; the new one does not.
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.fit_surface");
        h.frames(2);
        let top = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        h.step(&json!({"click": {"world": [31.0, 29.0, top(31.0, 29.0)]}})).unwrap();
        h.frames(2);
        assert!(gpu_fit_marks(&h.app).is_some(), "the point clicked on the scan");
        assert_eq!(selection_painted(&h), nothing, "Fit Surface draws the scan plain");
        h.app.preferences.free_form_plain_scan = false;
        h.frames(1);
        assert_ne!(selection_painted(&h), nothing, "the point's scan painted as selected, as before");
    }
    /// Crease Edge on: a click on the shared edge of two faces grown smoothly over a ridge creases
    /// it, so the faces follow the ridge; it is one step of Undo Last Change, which puts the
    /// tangency back; Remove Crease makes it smooth again; OK keeps the crease, which reads back
    /// from the angle the faces meet at. No point can be dragged while a crease mode is on.
    #[test]
    fn crease_edge_in_edit_surface_follows_the_ridge_and_remove_crease_smooths_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("ridge.stl");
        std::fs::write(&path, ridge_stl(60)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 6], [30, 10, 10], [30, 50, 10], [10, 50, 6]], "count": 5})).unwrap();
        h.app.run("scan.add_face", json!({"edge": [[30, 10, 10], [30, 50, 10]], "far": [[50, 10, 6], [50, 50, 6]], "rows": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let k = kind(&h).clone();
        assert_eq!((k.faces.len(), network::creased(&k.joins)), (2, vec![false]), "two faces, held tangent across the ridge");
        let before = k.fit.expect("the scan's distance").0;
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.creasing = Some(true);
        }
        let mid = k.joins.seams[0].a_side.at(&k.shown[k.joins.seams[0].a], 0.5).unwrap();
        // No drag moves a point while the mode is on.
        let dot = k.shown_dots.iter().find(|d| d.at.dist(mid) > 5.0).map(|d| d.at).unwrap();
        let to = dot + Vec3::new(0.0, 5.0, 0.0);
        h.step(&json!({"drag": [{"world": [dot.x, dot.y, dot.z]}, {"world": [to.x, to.y, to.z]}], "steps": 4})).unwrap();
        h.frames(2);
        assert!(kind(&h).history.is_empty() && kind(&h).moved.iter().all(|m| !*m), "no drag while Crease Edge is on");
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!((network::creased(&k.joins), k.history.len(), k.creases.len()), (vec![true], 1, 1), "creased in one step: {:?}", k.note);
        assert!(network::seam_angle(&k.faces, &k.joins.seams[0]) > 15.0, "the faces meet at an angle now");
        assert!(k.fit.is_some_and(|f| f.0 < before / 2.0), "and follow the ridge: {:?} against {before}", k.fit);
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            undo_last(e);
            assert_eq!((network::creased(&e.joins), e.creases.len()), (vec![false], 0), "the tangency back");
            assert!(network::worst_kink(&e.faces, &e.joins) < 1.0);
        }
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.creasing = Some(false);
        }
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!((network::creased(&k.joins), k.smooths.len()), (vec![false], 1), "smooth again: {:?}", k.note);
        assert!(network::worst_kink(&k.faces, &k.joins) < 1.0, "{}", network::worst_kink(&k.faces, &k.joins));
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.creasing = Some(true);
        }
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}})).unwrap();
        h.frames(2);
        assert_eq!(network::creased(&kind(&h).joins), vec![true]);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let kept = edit::editable(&h.app.session, Some("FreeForm1"), "test").unwrap();
        assert_eq!(network::creased(&network::joins(&kept.faces).unwrap()), vec![true], "the crease kept by the angle");
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert_eq!(network::creased(&kind(&h).joins), vec![true], "read back as a crease");
        // A double-click with Remove Crease on: the edge and those it runs on into (here, itself)
        // made smooth in one step, the first click of it taken back.
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.creasing = Some(false);
        }
        h.step(&json!({"click": {"world": [mid.x, mid.y, mid.z]}, "double": true})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!((network::creased(&k.joins), k.history.len()), (vec![false], 1), "one step: {:?}", k.note);
    }
    /// With the Smoothing Brush on, a drag across a point pushed up off the dome brings it back
    /// toward its neighbours in one change (the stroke), which Undo Last Change takes back whole;
    /// with Snap on the points stay on the scan; a click is a dab; the corners never move.
    #[test]
    fn the_smoothing_brush_evens_out_what_a_stroke_crosses_in_one_change() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let dome = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        let middle = 2 * 5 + 2;
        let height = |k: &EditSurface| k.shown_dots[middle].at.z - dome(k.shown_dots[middle].at.x, k.shown_dots[middle].at.y);
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.snap = SNAP_OFF;
            let up = k.shown_dots[middle].at + Vec3::new(0.0, 0.0, 3.0);
            move_to(k, middle, up);
            assert!(height(k) > 2.9, "pushed up: {}", height(k));
            k.brushing = true;
            k.brush_size = 90.0;
            k.brush_strength = 1.0;
            k.snap = SNAP_POINTER;
        }
        let k = kind(&h).clone();
        let (from, to) = (k.shown_dots[middle].at + Vec3::new(-12.0, 0.0, 0.0), k.shown_dots[middle].at + Vec3::new(12.0, 0.0, 0.0));
        let corner = k.shown_dots[0].at;
        let steps = k.history.len();
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 8})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert!(!k.stroke && k.drag.is_none(), "the stroke over");
        assert!(height(&k) < 2.0, "brushed down: {}", height(&k));
        assert_eq!(k.history.len(), steps + 1, "one change");
        assert!(k.shown_dots[0].at.dist(corner) < 1e-6, "the corner stays");
        assert!(k.fit.is_some_and(|f| f.0 < 0.5), "kept on the scan: {:?}", k.fit);
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            undo_last(e);
            assert!((height(e) - 3.0).abs() < 0.3, "the whole stroke taken back: {}", height(e));
        }
        // A click is a dab: one step, one change.
        let at = kind(&h).shown_dots[middle].at;
        h.step(&json!({"click": {"world": [at.x, at.y, at.z]}})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!(k.history.len(), steps + 1, "one change");
        assert!(height(&k) < 2.9, "a dab smooths a little: {}", height(&k));
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
    }
    /// Several points chosen at once: a click chooses one, Shift-click adds one, a box drawn on
    /// nothing chooses those inside it, and a drag of one chosen point moves them all by the same
    /// amount; a soft selection takes its neighbours along, less the further they are.
    #[test]
    fn several_points_can_be_chosen_and_dragged_together() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 6})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.snap = SNAP_OFF;
        }
        let dots = kind(&h).shown_dots.clone();
        let world = |i: usize| json!({"world": [dots[i].at.x, dots[i].at.y, dots[i].at.z]});
        let (a, b) = (2 * 6 + 2, 3 * 6 + 3);
        h.step(&json!({"click": world(a)})).unwrap();
        assert_eq!(kind(&h).picked.iter().copied().collect::<Vec<_>>(), vec![a], "a click chooses one");
        h.step(&json!({"click": world(b), "shift": true})).unwrap();
        assert_eq!(kind(&h).picked.iter().copied().collect::<Vec<_>>(), vec![a, b], "Shift adds one");
        h.step(&json!({"click": world(a), "ctrl": true})).unwrap();
        assert_eq!(kind(&h).picked.iter().copied().collect::<Vec<_>>(), vec![b], "Ctrl takes one away");
        // A box from outside the net to between the dots (a, b chosen again by the box).
        let corner = |x: f64, y: f64| json!({"world": [x, y, 0.0]});
        let (lo, hi) = (dots[a].at, dots[b].at);
        let (x0, y0, x1, y1) = (lo.x.min(hi.x) - 1.0, lo.y.min(hi.y) - 1.0, lo.x.max(hi.x) + 1.0, lo.y.max(hi.y) + 1.0);
        h.step(&json!({"drag": [corner(x0, y0), corner(x1, y1)], "steps": 4})).unwrap();
        h.frames(2);
        let k = kind(&h);
        assert!(k.marquee.is_none() && k.drag.is_none(), "the box is over");
        assert!(k.picked.contains(&a) && k.picked.contains(&b), "the box chose the points inside it: {:?}", k.picked);
        assert!(k.picked.len() < dots.len(), "but not all");
        // Dragging one chosen point moves all of them by the same amount.
        let chosen: Vec<usize> = k.picked.iter().copied().collect();
        let (from, to) = (dots[a].at, dots[a].at + Vec3::new(0.0, 4.0, 0.0));
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 6})).unwrap();
        h.frames(2);
        let after = kind(&h).clone();
        for &i in &chosen {
            assert!(after.moved[i], "point {i} moved by hand");
        }
        let rise = |i: usize| after.shown_dots[i].at.y - dots[i].at.y;
        assert!(
            chosen.iter().all(|&i| (rise(i) - rise(a)).abs() < 0.3),
            "they moved together: {:?}",
            chosen.iter().map(|&i| rise(i)).collect::<Vec<_>>()
        );
        assert!(rise(a) > 2.0, "and moved: {}", rise(a));
        // Choosing all, then none.
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            select::all(k);
            assert_eq!(k.picked.len(), 36);
            select::clear(k);
            assert!(k.picked.is_empty() && k.chosen.is_none());
        }
    }

    /// A click on a line of the grid chooses its two points and a double-click the whole row; a
    /// drag on a line moves it; the guides show through the choice and a drag on one moves the
    /// choice along its axis only.
    #[test]
    fn lines_are_chosen_and_dragged_and_the_guides_move_along_an_axis() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 6})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let dots = kind(&h).shown_dots.clone();
        let world = |p: Vec3| json!({"world": [p.x, p.y, p.z]});
        let mid = |a: usize, b: usize| (dots[a].at + dots[b].at) * 0.5;
        let picked = |h: &crate::scenario::Harness| kind(h).picked.iter().copied().collect::<Vec<_>>();
        // A double-click on a line chooses its whole row (first: clicks just before it would
        // make it a triple click).
        let (a, b) = (2 * 6 + 2, 2 * 6 + 3);
        h.step(&json!({"click": world(mid(a, b)), "double": true})).unwrap();
        assert_eq!(picked(&h), (12..18).collect::<Vec<_>>(), "a double-click chooses the whole row");
        // The guides go through the middle of the choice; a drag on the Y guide moves the row along
        // Y only, though the pointer goes across too.
        let k = kind(&h);
        assert!(guides::shown(k));
        let centre = select::centre(k).unwrap();
        // Two thirds out along the guide's solid part, past the choice at its middle.
        let proj = crate::viewport::projection(&h.app, h.app.viewport.rect.unwrap());
        let on_y = centre + Vec3::new(0.0, crate::gizmo::mm_per_px(&proj, centre).unwrap() * 45.0, 0.0);
        let to = on_y + Vec3::new(3.0, 3.0, 0.0);
        h.step(&json!({"drag": [world(on_y), world(to)], "steps": 6})).unwrap();
        h.frames(2);
        let after = kind(&h).clone();
        for (i, dot) in dots.iter().enumerate().take(18).skip(12) {
            let d = after.shown_dots[i].at - dot.at;
            assert!(d.x.abs() < 0.3 && (d.y - 3.0).abs() < 0.5, "point {i} went along Y only: {d:?}");
            assert!(after.moved[i], "point {i} moved by hand");
        }
        // A click between two dots of a row chooses both.
        let (c, e) = (4 * 6 + 1, 4 * 6 + 2);
        let at = (after.shown_dots[c].at + after.shown_dots[e].at) * 0.5;
        h.step(&json!({"click": world(at)})).unwrap();
        assert_eq!(picked(&h), vec![c, e], "a click on a line chooses its ends");
        // A drag on a line not chosen chooses it and moves it.
        h.step(&json!({"click": world(mid(0, 1))})).unwrap();
        let from = (after.shown_dots[c].at + after.shown_dots[e].at) * 0.5;
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.snap = SNAP_OFF;
        }
        h.step(&json!({"drag": [world(from), world(from + Vec3::new(0.0, -2.0, 0.0))], "steps": 6})).unwrap();
        h.frames(2);
        let last = kind(&h).clone();
        assert_eq!(last.picked.iter().copied().collect::<Vec<_>>(), vec![c, e], "the line dragged is the choice");
        for i in [c, e] {
            let d = last.shown_dots[i].at - after.shown_dots[i].at;
            assert!((d.y + 2.0).abs() < 0.5, "point {i} went with the line: {d:?}");
        }
    }

    /// The scan under the surface is looked for again when the scan moves, and when the faces are
    /// let go well away from where it was looked for, so the colouring follows both.
    #[test]
    fn the_deviation_follows_the_scan_moved_and_the_surface_moved_far() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[16, 16, 0], [32, 16, 0], [32, 32, 0], [16, 32, 0]], "count": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let middle = |k: &EditSurface| {
            let sh = k.shade.as_ref().expect("the scan under the surface");
            assert!(!sh.shade.points.is_empty());
            sh.shade.points.iter().fold(Vec3::ZERO, |a, p| a + p.at) * (1.0 / sh.shade.points.len() as f64)
        };
        let before = middle(kind(&h));
        let scan = kind(&h).shade.as_ref().unwrap().scan.clone();
        // The scan moved 3 mm up: the points coloured are the scan's where it is now.
        h.app.run("solid.move", json!({"bodies": [scan], "translate": [0, 0, 3]})).unwrap();
        h.frames(3);
        let up = middle(kind(&h)) - before;
        assert!((up.z - 3.0).abs() < 0.5, "the coloured scan went up with the scan: {up:?}");
        // The whole surface moved 20 mm along X: the scan it now lies over is coloured.
        let before = middle(kind(&h));
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            let places: Vec<Vec3> = k.shown_net.iter().map(|p| *p + Vec3::new(20.0, 0.0, 0.0)).collect();
            let faces = network::with_network_points(&k.shown, &k.joins, &places).unwrap();
            k.faces = faces.clone();
            k.net = places;
            show_faces(k, faces, false, None);
            assert!(k.restudy, "moved well away, the scan is looked for again");
        }
        h.frames(3);
        let k = kind(&h);
        assert!(!k.restudy && !outgrown(k));
        let over = middle(k) - before;
        assert!(over.x > 15.0, "the coloured scan is where the surface went: {over:?}");
    }

    /// A whole line goes on across a side two faces share, to the network's edge, and stops at
    /// the edge it runs into square.
    #[test]
    fn a_whole_line_runs_on_across_the_faces() {
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
        let k = kind(&h).clone();
        let (f, side) =
            *k.joins.open.iter().find(|&&(f, s)| s.at(&k.shown[f], 0.5).is_some_and(|p| (p.x - 28.0).abs() < 1.0)).expect("the edge at x = 28");
        let edge = side.at(&k.shown[f], 0.5).unwrap();
        h.step(&json!({"click": {"world": [edge.x, edge.y, edge.z]}})).unwrap();
        h.frames(2);
        let top = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        for (x, y) in [(38.0, 20.0), (38.0, 40.0)] {
            h.step(&json!({"click": {"world": [x, y, top(x, y)]}})).unwrap();
            h.frames(2);
        }
        let k = kind(&h).clone();
        assert_eq!(k.shown.len(), 2, "a face added: {:?}", k.note);
        // A step of the first face's grid that crosses toward the shared side, in its middle.
        let across = select::grid_lines(&k)
            .into_iter()
            .find(|l| {
                let (p, q) = (k.shown_dots[l.ends[0]].at, k.shown_dots[l.ends[1]].at);
                l.face == 0 && (p.y - q.y).abs() < 1e-6 && (p.y - 30.0).abs() < 1.0 && p.x < 24.0 && q.x < 24.0
            })
            .expect("a step along x in the middle of the first face");
        let whole = select::whole_line(&k, across.ends[0], across.ends[1]);
        let xs: Vec<f64> = whole.iter().map(|p| k.shown_dots[*p].at.x).collect();
        assert!(xs.iter().all(|x| x.is_finite()));
        let (lo, hi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        assert!(lo < 17.0 && hi > 37.0, "from one edge of the network to the other: {xs:?}");
        assert!(whole.iter().all(|p| (k.shown_dots[*p].at.y - 30.0).abs() < 1.0), "along the one row: {whole:?}");
        let ids: BTreeSet<usize> = whole.iter().copied().collect();
        assert_eq!(ids.len(), whole.len(), "each point once");
    }

    /// The shape tools on the points chosen: Constrain to a plane puts them all in one plane, a
    /// shape needing more points than are chosen says so, a pattern whose copies do not touch is
    /// refused in words, and Suggest Refinement answers either way.
    #[test]
    fn the_shape_tools_hold_chosen_points_to_a_plane_and_refuse_what_cannot_be_made() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 6})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) else { panic!("the dialog") };
        k.snap = SNAP_OFF;
        select::all(k);
        assert_eq!(k.picked.len(), 36);
        let flat_before = k.shown_dots.iter().map(|d| d.at.z).fold((f64::MAX, f64::MIN), |(lo, hi), z| (lo.min(z), hi.max(z)));
        shape::constrain_to(k, Some(solidcraft_engine::cmd::scan::surface::constrain::Kind::Plane), 0.0).unwrap();
        let zs: Vec<f64> = k.shown_dots.iter().map(|d| d.at.z).collect();
        let spread = zs.iter().cloned().fold(f64::MIN, f64::max) - zs.iter().cloned().fold(f64::MAX, f64::min);
        assert!(flat_before.1 - flat_before.0 > 1.0 && spread < 0.5, "the dome flattened: {flat_before:?} → {spread}");
        assert!(k.shape.note.is_some(), "what was held to is said");
        // Two points are too few for a cylinder.
        select::clear(k);
        select::pick_many(k, &[0, 1], select::How::Add);
        let few = shape::constrain_to(k, Some(solidcraft_engine::cmd::scan::surface::constrain::Kind::Cylinder), 0.0).unwrap_err();
        assert!(!few.is_empty(), "refused in words");
        // Copies that do not touch are refused, not crashed on.
        k.shape.step = 500.0;
        assert!(shape::repeated_for_test(k, false).is_err());
    }

    /// Symmetry chosen on a half modelled on one side of the dome's mirror plane mirrors it into
    /// one Free Form joined on the plane; a point moved takes its twin to the mirror image; Undo
    /// Last Change takes the mirroring back and turns Symmetry off; chosen again, OK keeps both
    /// halves as one body.
    #[test]
    fn symmetry_mirrors_a_half_and_an_edit_moves_both_sides() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [29.6, 10, 0], [29.6, 50, 0], [10, 50, 0]], "count": 5})).unwrap();
        h.app.run("scan.symmetry", json!({"plane": {"point": [30, 30, 8], "normal": [1, 0, 0]}})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let planes = symmetry_planes(&h.app, kind(&h));
        let kept = planes.iter().find(|(n, _)| n == "Symmetry Plane").cloned().expect("Find Symmetry's plane offered");
        assert_eq!((kind(&h).symmetry, planes.len()), (0, 4), "the three origin planes and the kept one");
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.symmetry = 4;
            apply_symmetry(k, Some(kept.clone()));
        }
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!((k.faces.len(), k.history.len()), (2, 1), "mirrored in one step: {:?}", k.mirror_note);
        let pairs = k.mirror.clone().expect("paired");
        assert_eq!((pairs.pairs(), pairs.on_plane_count()), (20, 5));
        assert!(network::worst_kink(&k.faces, &k.joins) < 0.5, "smooth across the plane");
        // A dot on the half raised 3 mm: its twin rises to the mirror image.
        let inner = (0..k.shown_dots.len())
            .find(|&p| !pairs.is_on_plane(p) && (k.shown_dots[p].at.x - 20.0).abs() < 2.0 && (k.shown_dots[p].at.y - 30.0).abs() < 2.0)
            .unwrap();
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.snap = SNAP_OFF;
            let up = e.shown_dots[inner].at + Vec3::new(0.0, 0.0, 3.0);
            move_to(e, inner, up);
            let twin = pairs.twin[inner];
            let (a, b) = (e.shown_dots[inner].at, e.shown_dots[twin].at);
            assert!(a.z > up.z - 0.5 && b.dist(pairs.plane.reflect(a)) < 1e-6, "both sides: {a:?} {b:?}");
            // A point on the plane sent sideways stays on it.
            let on = (0..e.shown_dots.len()).find(|&p| pairs.is_on_plane(p)).unwrap();
            let to = e.shown_dots[on].at + Vec3::new(4.0, 0.0, 1.0);
            move_to(e, on, to);
            assert!(pairs.plane.side(e.shown_dots[on].at).abs() < 1e-6, "on the plane still");
            // Undo Last Change back past the mirroring: one face, Symmetry off.
            undo_last(e);
            undo_last(e);
            undo_last(e);
            assert_eq!((e.faces.len(), e.symmetry, e.mirror.is_some()), (1, 0, false), "{:?}", e.mirror_note);
            e.symmetry = 4;
            apply_symmetry(e, Some(kept));
            assert_eq!(e.faces.len(), 2);
        }
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let body = edit::editable(&h.app.session, Some("FreeForm1"), "test").unwrap();
        assert_eq!(body.faces.len(), 2, "both halves kept as one body");
        let joins = network::joins(&body.faces).unwrap();
        let tolerance = symmetric::pairing_tolerance(&body.faces, 1.0);
        assert!(symmetric::pairs_of(&body.faces, &joins, kind_plane(&h), tolerance).is_ok(), "a mirror image as kept");
        assert!(h.app.session.doc.free_forms.first().is_some_and(|k| k.symmetry.is_some()), "the design keeps the plane");

        // Opened again: Symmetry is on in the plane kept.
        h.app.start("scan.edit_surface");
        h.frames(3);
        let k = kind(&h).clone();
        assert!(k.mirror.is_some() && k.symmetry == 4, "Symmetry on as kept: {} {:?}", k.symmetry, k.mirror_note);
        // None: both halves stay, and Keep One Half is offered; a click on a face of the -x half
        // keeps that half.
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            e.symmetry = 0;
            apply_symmetry(e, None);
            assert!(e.half_offer.is_some() && e.faces.len() == 2, "both halves stay, one can go");
            e.keeping_half = true;
        }
        let top = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        h.step(&json!({"click": {"world": [20.0, 30.0, top(20.0, 30.0)]}})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!((k.faces.len(), k.half_offer.is_none(), k.history.len()), (1, true, 1), "one half kept: {:?}", k.note);
        assert!(edit::control_points(&k.faces[0]).iter().all(|p| p.x <= 30.0 + 1e-6), "the -x half");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(edit::editable(&h.app.session, Some("FreeForm1"), "test").unwrap().faces.len(), 1);
        assert!(h.app.session.doc.free_forms.is_empty(), "the plane let go");
    }

    /// The plane Find Symmetry kept, as Edit Surface reads it.
    fn kind_plane(h: &crate::scenario::Harness) -> MirrorPlane {
        symmetry_planes(&h.app, &EditSurface::default()).into_iter().find(|(n, _)| n == "Symmetry Plane").map(|(_, p)| p).unwrap()
    }

    /// A corner of a face on the Free Form's edge dragged onto a neighbour's corner is welded to
    /// it when let go: the two open sides whose ends then meet are sewn into one shared edge, in
    /// one step of Undo Last Change, and OK keeps the four faces as one network.
    #[test]
    fn a_corner_dropped_on_a_corner_is_welded_and_the_sides_sewn() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        // In the middle of the view: the top of a top view is under the ribbon.
        h.app.run("scan.free_form", json!({"corners": [[10, 6, 0], [30, 6, 0], [30, 24, 0], [10, 24, 0]], "count": 5})).unwrap();
        h.app.run("scan.add_face", json!({"edge": [[30, 6, 0], [30, 24, 0]], "far": [[50, 6, 0], [50, 24, 0]], "rows": 5})).unwrap();
        h.app.run("scan.add_face", json!({"edge": [[10, 24, 0], [30, 24, 0]], "far": [[10, 42, 0], [30, 42, 0]], "rows": 5})).unwrap();
        h.app.run("scan.add_face", json!({"edge": [[30, 24, 0], [50, 24, 0]], "far": [[34, 45, 0], [50, 42, 0]], "rows": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        let k = kind(&h).clone();
        assert_eq!((k.faces.len(), k.joins.seams.len()), (4, 3));
        let corner_near = |k: &EditSurface, q: Vec3| {
            k.joins.corners.iter().filter(|c| c.border).map(|c| k.shown_net[c.point]).min_by(|a, b| a.dist(q).total_cmp(&b.dist(q))).unwrap()
        };
        let (from, onto) = (corner_near(&k, Vec3::new(34.0, 45.0, 7.0)), corner_near(&k, Vec3::new(30.0, 42.0, 7.0)));
        assert!(from.dist(onto) > 2.0, "apart to begin with");
        // The corners in the middle of the view, clear of the ribbon.
        h.app.cam.target = (from + onto) * 0.5;
        h.frames(2);
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [onto.x, onto.y, onto.z]}], "steps": 6})).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert_eq!((k.faces.len(), k.joins.seams.len(), k.history.len()), (4, 4, 1), "sewn in one step: {:?}", k.note);
        assert!(k.told.is_some(), "and said so");
        assert!(k.joins.corners.iter().any(|c| c.faces == 4 && !c.border), "the middle a grid's corner");
        if let Some(Kind::ScanEditSurface(e)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            undo_last(e);
            assert_eq!(e.joins.seams.len(), 3, "taken back whole");
        }
        h.step(&json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [onto.x, onto.y, onto.z]}], "steps": 6})).unwrap();
        h.frames(2);
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let body = edit::editable(&h.app.session, Some("FreeForm1"), "test").unwrap();
        assert_eq!(network::joins(&body.faces).unwrap().seams.len(), 4, "kept sewn");
    }

    /// With the preference on (the default), Ctrl+Shift and a drag smooths with the brush off,
    /// and Ctrl+Shift with the wheel sizes the brush instead of zooming; off, neither does.
    #[test]
    fn ctrl_shift_brushes_and_sizes_the_brush_unless_the_preference_is_off() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("dome.stl");
        std::fs::write(&path, dome_stl(40)).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 5})).unwrap();
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.app.start("scan.edit_surface");
        h.frames(3);
        assert!(h.app.preferences.edit_surface_brush_keys, "on by default");
        let dome = |x: f64, y: f64| 8.0 - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2));
        let middle = 2 * 5 + 2;
        let height = |k: &EditSurface| k.shown_dots[middle].at.z - dome(k.shown_dots[middle].at.x, k.shown_dots[middle].at.y);
        if let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.snap = SNAP_OFF;
            let up = k.shown_dots[middle].at + Vec3::new(0.0, 0.0, 3.0);
            move_to(k, middle, up);
            k.brush_size = 90.0;
            k.brush_strength = 1.0;
            k.snap = SNAP_POINTER;
        }
        let k = kind(&h).clone();
        assert!(!k.brushing && height(&k) > 2.9);
        let steps = k.history.len();
        let (from, to) = (Vec3::new(2.0, 30.0, dome(2.0, 30.0)), Vec3::new(58.0, 30.0, dome(58.0, 30.0)));
        let stroke = json!({"drag": [{"world": [from.x, from.y, from.z]}, {"world": [to.x, to.y, to.z]}], "steps": 10, "ctrl": true, "shift": true});
        h.step(&stroke).unwrap();
        h.frames(2);
        let k = kind(&h).clone();
        assert!(height(&k) < 2.0 && k.history.len() == steps + 1 && !k.brushing, "brushed in one change, the brush still off: {}", height(&k));
        // Ctrl+Shift and the wheel: the circle grows, the view stays.
        let middle_of_view = h.app.viewport.rect.expect("the view").center();
        let cam = h.app.cam;
        h.call("ui.scroll", json!({"x": middle_of_view.x, "y": middle_of_view.y, "delta": -200, "ctrl": true, "shift": true}));
        h.frames(2);
        let size = kind(&h).brush_size;
        assert!((size - 90.0).abs() > 5.0, "sized: {size}");
        assert!((h.app.cam.distance - cam.distance).abs() < 1e-9, "no zoom");
        // The old behaviour, kept behind the preference: the wheel zooms, a drag does not brush.
        h.app.preferences.edit_surface_brush_keys = false;
        h.call("ui.scroll", json!({"x": middle_of_view.x, "y": middle_of_view.y, "delta": -200, "ctrl": true, "shift": true}));
        h.frames(2);
        assert_eq!(kind(&h).brush_size, size, "not sized");
        let steps = kind(&h).history.len();
        h.step(&stroke).unwrap();
        h.frames(2);
        assert_eq!(kind(&h).history.len(), steps, "no stroke");
    }

    /// A double-click on a Free Form in the view opens Edit Surface on that very surface, even with
    /// a later one made; nothing opens on a body that is no editable surface.
    #[test]
    fn double_clicking_a_free_form_edits_it() {
        let mut h = crate::scenario::Harness::new();
        for corners in [[[0, 0, 0], [20, 0, 0], [20, 20, 0], [0, 20, 0]], [[40, 0, 0], [60, 0, 0], [60, 20, 0], [40, 20, 0]]] {
            h.app.run("scan.free_form", json!({"corners": corners, "plane": "XY"})).unwrap();
        }
        h.step(&json!({"view_anim": "top"})).unwrap();
        h.frames(3);
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        h.step(&json!({"click": {"world": [10.0, 10.0, 0.0]}, "double": true})).unwrap();
        h.frames(3);
        let Some(Kind::ScanEditSurface(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("Edit Surface should be open") };
        assert_eq!(k.target.as_deref(), Some("FreeForm1"), "the one clicked, not the one made last");
    }
}
