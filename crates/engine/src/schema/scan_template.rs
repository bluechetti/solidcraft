//! Contract and result type for `scan.flat_template` (`cmd/scan_template.rs`): a flat template of
//! a wall from its scan, sketched with a cut-out for each part standing off it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::Point3;

/// A plane given by a point on it and the way it faces.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct TemplatePlane {
    pub point: Point3,
    pub normal: Point3,
}

/// Make a flat template of a wall from its scan, as a sketch on a plane through the wall: the outline picked, laid flat
/// on the wall's plane, a cut-out for each part that stands off the plane, and the scan's own edge
/// as construction lines. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFlatTemplateParams {
    /// The scan. Default: the picked, selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The outline: three or more points [x, y, z] in order round it, put onto the plane exactly
    /// (nothing else snaps to the scan), or `scan` for the whole wall's outline found from the
    /// scan (the plane it rests on, tears narrower than `gap` bridged, each part standing off it a
    /// notch or a cut-out).
    pub outline: TemplateOutline,
    /// The wall's plane. Default: fitted to the scan under the outline, the parts standing far off
    /// it left out.
    #[serde(default)]
    pub plane: Option<TemplatePlane>,
    /// How far off the plane a part must stand to be cut out. Default: four times the wall's
    /// noise, at least 1 mm.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// Which parts are cut out: `both` (default), `out` (on the side the parts stand farthest off,
    /// or the plane's normal when `plane` is given) or `in`.
    #[serde(default)]
    pub side: Option<String>,
    /// The least area of a cut-out, in mm². Default: 25.
    #[serde(default)]
    pub least_area: Option<f64>,
    /// Draw the scan's own edge as construction lines. Default: true.
    #[serde(default)]
    pub show_scan_edge: Option<bool>,
    /// Also make a flat Free Form face over the template, each cut-out a hole in it. Default:
    /// false.
    #[serde(default)]
    pub make_face: Option<bool>,
    /// The sketch's name. Default: `Wall Template`.
    #[serde(default)]
    pub name: Option<String>,
    /// With `outline: "scan"`: tears and notches in the scan's edge narrower than this are bridged.
    /// Default: 50 mm.
    #[serde(default)]
    pub gap: Option<f64>,
    /// With `outline: "scan"`: the grid the outline is found on, and so how true it is. Default:
    /// 2 mm (larger for a very large scan).
    #[serde(default)]
    pub step: Option<f64>,
    /// With `outline: "scan"`: notches in the outline, cut-outs and slivers narrower than this are
    /// smoothed away, so the template goes over a cable or a bracket rather than round it. Default:
    /// 100 mm; 0 keeps them all.
    #[serde(default)]
    pub smooth: Option<f64>,
    /// With `outline: "scan"`: the template tidied: sides nearly level or plumb made exactly so,
    /// those nearly on one line put on it, and every corner rounded to this grid from the template's
    /// corner, no side added or taken away. Default: 5 mm; 0 leaves it as found.
    #[serde(default)]
    pub round: Option<f64>,
    /// With `outline: "scan"`: fingers of the template narrower than this reaching more than
    /// 100 mm in between parts standing out (a tongue between a raised corner and a ridge, a lobe
    /// into a pocket in a raised frame) are cut off with them. Default: 300 mm; 0 keeps them.
    #[serde(default)]
    pub finger: Option<f64>,
    /// With `outline: "scan"`: places on raised parts to cover rather than cut round (up to 100):
    /// at each, the raised part there, as far as it stays within 15 mm of its height there (a
    /// plateau and a low ridge on it, not the block it rises steeply to).
    #[serde(default)]
    pub cover: Option<Vec<Point3>>,
    /// With `outline: "scan"`: a straight panel cut from sheets instead of the outline as found:
    /// straight edges, the cut-outs they need, seams along the frame's rows of holes and the holes
    /// near the edges and seams, each kept unless dropped here (see `scan.panel_proposals`).
    #[serde(default)]
    pub panel: Option<PanelChoices>,
}

/// What is kept of a straight panel's proposal (`scan.panel_proposals` lists it, numbered).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PanelChoices {
    /// The sheet's size, long × short (mm). Default: [2440, 1220] (8 × 4 ft).
    #[serde(default)]
    pub sheet: Option<[f64; 2]>,
    /// The straight edges, in mm from the bottom-left corner of the wall's outline as found.
    /// Default: each where the outline lies beyond it along most of that side.
    #[serde(default)]
    pub edges: Option<PanelEdges>,
    /// Cut-outs not cut (the panel goes over them), by their number in the proposal.
    #[serde(default)]
    pub drop_cutouts: Option<Vec<usize>>,
    /// Seams not made (the pieces either side are one), by their number.
    #[serde(default)]
    pub drop_seams: Option<Vec<usize>>,
    /// Holes not drawn, by their number.
    #[serde(default)]
    pub drop_holes: Option<Vec<usize>>,
    /// Seams moved square to themselves (mm; up or right is positive).
    #[serde(default)]
    pub move_seams: Option<Vec<SeamMove>>,
    /// Look for the frame's holes. Default: true.
    #[serde(default)]
    pub holes: Option<bool>,
    /// Which way is up along the wall, a direction [x, y, z]: the panel is turned in its plane so
    /// its bottom edge is the floor. Default: as the wall's plane was found.
    #[serde(default)]
    pub up: Option<Point3>,
    /// Quarter turns counter-clockwise in its plane after `up` (0 to 3), to put the floor at the
    /// bottom edge. Default: 0.
    #[serde(default)]
    pub turn: Option<u8>,
    /// The gap left at every edge, round every cut-out and at every seam (mm). Default: 3.
    #[serde(default)]
    pub clearance: Option<f64>,
    /// How often the edges are fastened (mm): a stretch longer than this with no hole near gets
    /// fixing points on flat metal behind it. Default: 300; 0 proposes none.
    #[serde(default)]
    pub spacing: Option<f64>,
    /// Propose the frame's access holes (20 mm and up) too. Default: false.
    #[serde(default)]
    pub access_holes: Option<bool>,
    /// The sizes the frame's holes come in (mm): each hole is drawn at the nearest within 1 mm of
    /// it as scanned. Default: as scanned.
    #[serde(default)]
    pub hole_sizes: Option<Vec<f64>>,
    /// The diameter a fixing point is drawn (mm). Default: 7.
    #[serde(default)]
    pub fixing_hole: Option<f64>,
    /// Cut-outs cut another way than proposed, by their number: `box`, `straight` or `scanned`.
    #[serde(default)]
    pub shapes: Option<Vec<CutoutShape>>,
    /// Fixing points not made, by their number.
    #[serde(default)]
    pub drop_fixings: Option<Vec<usize>>,
    /// Another scan of the wall (or of the wall opposite, its mirror image): each hole is checked
    /// against it, `both` when it has the hole too, `not_found` when it covers the place without
    /// one (likely a gap in this scan, not a hole), `unscanned` when it does not cover it.
    #[serde(default)]
    pub confirm_with: Option<String>,
    /// Holes the other scan covers without a hole there are not drawn. Default: false.
    #[serde(default)]
    pub drop_unconfirmed: Option<bool>,
    /// What the panel is squared to: `holes` (the rows and columns of the frame's holes),
    /// `outline` (the way the outline's sides run) or `auto` (the holes when they agree the wall
    /// is turned 0.25° or more on the outline's sides). Default: auto.
    #[serde(default)]
    pub square_to: Option<String>,
    /// Openings in the wall's metal behind the panel of this area or more (mm²) are traced, for
    /// pieces cut to drop into them; 0 traces none. Default: 50 000 (about 220 × 220 mm).
    #[serde(default)]
    pub insert_least: Option<f64>,
    /// Openings not cut pieces for, by their number.
    #[serde(default)]
    pub drop_inserts: Option<Vec<usize>>,
}

/// One cut-out's shape: its number and `box`, `straight` (dents filled, straight sides) or
/// `scanned` (as found).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct CutoutShape {
    pub cutout: usize,
    pub shape: String,
}

/// A straight panel's edges (mm from the bottom-left corner of the wall's outline as found).
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct PanelEdges {
    #[serde(default)]
    pub left: Option<f64>,
    #[serde(default)]
    pub right: Option<f64>,
    #[serde(default)]
    pub bottom: Option<f64>,
    #[serde(default)]
    pub top: Option<f64>,
}

/// One seam moved: its number and how far (mm).
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct SeamMove {
    pub seam: usize,
    pub by: f64,
}

/// List what a straight panel cut from sheets would have, for checking one by one before
/// `scan.flat_template` with `outline: "scan"` and `panel` makes it. Changes nothing.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPanelProposalsParams {
    /// The scan. Default: the picked, selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The wall's plane. Default: the plane it rests on (its outermost large flat face).
    #[serde(default)]
    pub plane: Option<TemplatePlane>,
    /// How far off the plane a part must stand to be cut round. Default: four times the wall's
    /// noise, at least 1 mm.
    #[serde(default)]
    pub tolerance: Option<f64>,
    /// Which parts are cut round: `out` (default), `in` or `both`.
    #[serde(default)]
    pub side: Option<String>,
    /// As for `scan.flat_template`.
    #[serde(default)]
    pub least_area: Option<f64>,
    #[serde(default)]
    pub gap: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub smooth: Option<f64>,
    #[serde(default)]
    pub round: Option<f64>,
    #[serde(default)]
    pub finger: Option<f64>,
    #[serde(default)]
    pub cover: Option<Vec<Point3>>,
    /// The sheet, the edges and what is dropped or moved, as for `scan.flat_template`.
    #[serde(default)]
    pub panel: Option<PanelChoices>,
}

/// Write a straight panel's pieces for cutting: each piece's outline, cut-outs, holes and fixing
/// points at its own 0, 0, labelled and dimensioned (DXF), the blanks laid on sheets (DXF), every
/// hole and fixing point (CSV), the cut list (CSV), distances to check in the van first (CSV) and
/// the drawings (PDF: the panel, a page for each piece, the sheets).
/// Changes nothing in the design.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanPanelExportParams {
    /// The pieces' DXF; the other files are written beside it, named after it. Default: none, the
    /// files' text is returned.
    #[serde(default)]
    pub path: Option<String>,
    /// As for `scan.panel_proposals`.
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub plane: Option<TemplatePlane>,
    #[serde(default)]
    pub tolerance: Option<f64>,
    #[serde(default)]
    pub side: Option<String>,
    #[serde(default)]
    pub least_area: Option<f64>,
    #[serde(default)]
    pub gap: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub smooth: Option<f64>,
    #[serde(default)]
    pub round: Option<f64>,
    #[serde(default)]
    pub finger: Option<f64>,
    #[serde(default)]
    pub cover: Option<Vec<Point3>>,
    #[serde(default)]
    pub panel: Option<PanelChoices>,
    /// The full-size print's paper: `letter` (default), `a4` or `none` for no print.
    #[serde(default)]
    pub paper: Option<String>,
}

/// A piece as exported.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelPieceOut {
    /// Numbered from 1, top row first, left to right.
    pub number: usize,
    /// Its blank (mm), and the sheet it is cut from (from 1).
    pub blank: [f64; 2],
    pub sheet: usize,
    pub holes: usize,
    pub fixings: usize,
}

/// A distance to measure in the van before cutting, between two holes (piece and hole numbers).
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelCheckOut {
    pub from_piece: usize,
    pub from_hole: usize,
    pub to_piece: usize,
    pub to_hole: usize,
    /// Straight between the two, and how far across and up (mm).
    pub distance: f64,
    pub across: f64,
    pub up: f64,
}

/// What `scan.panel_export` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanPanelExportResult {
    /// The files written (with `path`).
    pub files: Vec<String>,
    pub pieces: Vec<PanelPieceOut>,
    pub sheets: usize,
    /// Pages in the drawings' PDF (written with `path`): the panel, one for each piece, the sheets.
    pub pdf_pages: usize,
    /// Pages in the full-size print (written with `path` as `<name>-full-size.pdf`): a cover, a
    /// map of each piece's pages when it has more than one, the pages at 1:1.
    pub full_size_pages: usize,
    pub checks: Vec<PanelCheckOut>,
    /// The pieces cut to drop into the openings behind the panel (numbered from 1, as drawn).
    pub inserts: Vec<PanelInsertOut>,
    /// Without `path`: the files' text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pieces_dxf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheets_dxf: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holes_csv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut_list_csv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks_csv: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inserts_csv: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// A piece cut to drop into an opening behind the panel.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelInsertOut {
    /// Its number in the drawings, and the opening's number in `scan.panel_proposals`.
    pub number: usize,
    pub opening: usize,
    /// Its box (mm, the gap already taken off) and how deep the opening is behind the panel (mm).
    pub size: [f64; 2],
    pub depth: f64,
    /// The corner radius of the rounded box nearest it, and the farthest its trace strays from
    /// that box (mm): under a few mm it can be cut as that box.
    pub corner_radius: f64,
    pub off_rounded_box: f64,
}

/// A cut-out proposed for a straight panel.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelCutoutProposal {
    /// A notch in an edge (rather than a cut-out inside), and the edges it opens on.
    pub notch: bool,
    pub edges: Vec<String>,
    /// How far it reaches in (mm), its area (mm²) and its box (mm).
    pub depth: f64,
    pub area: f64,
    pub size: [f64; 2],
    /// Its middle in space, and its outline in mm from the outline's corner.
    pub center: [f64; 3],
    pub ring: Vec<[f64; 2]>,
    /// How it is proposed to be cut: `box`, `straight` or `scanned`.
    pub shape: String,
}

/// A new place to fasten the panel: flat metal behind it where an edge had no hole near.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelFixingProposal {
    pub at: [f64; 2],
    pub center: [f64; 3],
    /// How far the metal is behind the panel's back face (mm; less than 0: proud of it).
    pub behind: f64,
}

/// An opening in the wall's metal behind the panel (a bay, the outer skin seen through the inner
/// sheet), traced at its lip, for a piece cut to drop into it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelInsertProposal {
    /// Its outline (mm from the outline's corner, counter-clockwise), area (mm²), box and middle.
    pub ring: Vec<[f64; 2]>,
    pub area: f64,
    pub size: [f64; 2],
    pub center: [f64; 3],
    /// How far its back lies behind the panel (the median, mm): how thick a piece fills it.
    pub depth: f64,
    /// It runs out to the panel's edge and is cut off there.
    pub at_edge: bool,
    /// The piece's box once the gap is taken off all round (mm).
    pub piece_size: [f64; 2],
}

/// A piece laid on a sheet.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelPlaced {
    /// The piece's number in `pieces`, where its corner goes on the sheet (mm), its size as laid
    /// and whether it is turned a quarter.
    pub piece: usize,
    pub at: [f64; 2],
    pub size: [f64; 2],
    pub turned: bool,
}

/// One sheet and the pieces cut from it.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelSheet {
    pub pieces: Vec<PanelPlaced>,
    /// Its one piece is too big for the sheet.
    pub too_big: bool,
}

/// A seam move that brings every piece within the sheet.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelFitMove {
    pub seam: usize,
    pub by: f64,
}

/// A seam proposed: a straight line across the panel where two sheets meet.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelSeamProposal {
    /// Up the panel (rather than across it).
    pub upright: bool,
    /// Between two rows of holes, so each sheet gets one (rather than beside one row).
    pub between_rows: bool,
    /// Its ends (mm from the outline's corner, and in space).
    pub from: [f64; 2],
    pub to: [f64; 2],
    pub from_point: [f64; 3],
    pub to_point: [f64; 3],
}

/// A hole of the frame proposed: one near an edge or a seam, to fasten the panel through.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelHoleProposal {
    /// Its centre (mm from the outline's corner, and in space) and diameter (mm, as scanned).
    pub at: [f64; 2],
    pub center: [f64; 3],
    pub diameter: f64,
    /// How far its rim is behind the panel's back face (mm; less than 0: proud of it).
    pub behind: f64,
    /// Its size as drawn: the nearest of `hole_sizes` within 1 mm, or as scanned.
    pub size: f64,
    /// With `confirm_with`: `both`, `not_found` or `unscanned`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen: Option<String>,
}

/// How the holes were checked against another scan.
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelConfirmOut {
    pub body: String,
    /// The two scans' holes lined up: mirrored (the wall opposite), how many matched and how far
    /// apart they were after (mm, root mean square). None lined up: `matched` 0.
    pub mirrored: bool,
    pub matched: usize,
    pub rms: f64,
    /// The proposal's holes the other scan has too, covers without, and does not cover.
    pub both: usize,
    pub not_found: usize,
    pub unscanned: usize,
}

/// A straight panel's edges (mm from the outline's corner).
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelEdgesOut {
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
    pub top: f64,
}

/// What `scan.panel_proposals` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanPanelProposalsResult {
    pub body: String,
    /// The wall's frame: where 0, 0 is in space (the outline's bottom-left corner), and its axes.
    pub origin: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    pub normal: [f64; 3],
    pub edges: PanelEdgesOut,
    /// Numbered from 0, in this order, for `drop_cutouts`, `drop_seams`, `drop_holes`.
    pub cutouts: Vec<PanelCutoutProposal>,
    pub seams: Vec<PanelSeamProposal>,
    pub holes: Vec<PanelHoleProposal>,
    pub fixings: Vec<PanelFixingProposal>,
    /// Numbered from 0 for `drop_inserts`, in rows from the top, left to right.
    pub inserts: Vec<PanelInsertProposal>,
    /// The frame's holes found in all (the proposal keeps those near an edge or a seam).
    pub holes_found: usize,
    /// Places an edge needed fastening but no flat metal was found behind it.
    pub unbacked: usize,
    /// The gap at edges, cut-outs and seams (mm).
    pub clearance: f64,
    /// The pieces laid on sheets, and seam moves that would bring every piece within the sheet.
    pub sheets: Vec<PanelSheet>,
    pub fit_moves: Vec<PanelFitMove>,
    /// With `confirm_with`: how the holes were checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<PanelConfirmOut>,
    /// When the panel was squared to the frame's holes: how far it turned from the outline's own
    /// squaring (degrees).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub squared_to_holes: Option<f64>,
    /// Notches too shallow to cut (the edge goes over them) and how deep the deepest is (mm).
    pub flattened: usize,
    pub flattened_depth: f64,
    /// The sheet, each piece's size (width × height, mm, top row first) and how far the longest
    /// piece is past the sheet (0: all fit).
    pub sheet: [f64; 2],
    pub pieces: Vec<[f64; 2]>,
    pub over: f64,
    /// The panel's outline with everything kept (mm from the outline's corner).
    pub outline: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// What a straight panel came to (`scan.flat_template` with `panel`).
#[derive(Debug, Serialize, JsonSchema)]
pub struct PanelMade {
    pub edges: PanelEdgesOut,
    pub seams: usize,
    pub holes: usize,
    pub fixings: usize,
    /// The openings behind the panel kept for pieces to drop into.
    pub inserts: usize,
    /// How many sheets the pieces are cut from.
    pub sheets: usize,
    pub pieces: Vec<[f64; 2]>,
    pub over: f64,
}

/// The points picked round a template, or `scan` for the whole outline found from the scan.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TemplateOutline {
    Points(Vec<Point3>),
    Found(OutlineFromScan),
}

/// The whole outline found from the scan.
#[derive(Debug, Deserialize, JsonSchema)]
pub enum OutlineFromScan {
    #[serde(rename = "scan")]
    Scan,
}

/// A part cut out of the template.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanTemplateCutout {
    /// The footprint's area (mm²) and how far the part stands off the plane at most (mm).
    pub area: f64,
    pub height: f64,
    /// `out` or `in`: the side of the plane it stands off.
    pub side: String,
    /// Its footprint's middle in space, and its size along the template's x and y.
    pub center: [f64; 3],
    pub size: [f64; 2],
    /// The part runs off the scan's edge (its footprint was closed with a straight line).
    pub at_scan_edge: bool,
    /// The footprint crosses the outline: clipped to it, or kept whole for an outline that is not
    /// convex.
    pub crosses: bool,
    pub clipped: bool,
}

/// What `scan.flat_template` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFlatTemplateResult {
    pub body: String,
    pub sketch: u64,
    /// The plane used: a point on it and its unit normal.
    pub point: [f64; 3],
    pub normal: [f64; 3],
    /// Whether the plane was fitted to the scan, the scan's points it was fitted to and how many
    /// were under the outline.
    pub fitted: bool,
    pub kept: usize,
    pub under: usize,
    /// The tolerance used (mm).
    pub tolerance: f64,
    /// The outline's area and the area the cut-outs take from it (mm²).
    pub area: f64,
    pub cut_area: f64,
    pub cutouts: Vec<ScanTemplateCutout>,
    /// Parts found, left out for being under the least area, and left out for lying outside.
    pub found: usize,
    pub specks: usize,
    pub outside: usize,
    /// Loops of the scan's edge drawn, and lines, arcs and circles drawn in all.
    pub scan_edge_loops: usize,
    pub lines: usize,
    pub arcs: usize,
    pub circles: usize,
    /// Where the template was laid: its own coordinates minus this (the outline's corner is the
    /// sketch's origin, on a plane through the wall).
    pub shift: [f64; 2],
    /// The Free Form body made (with `make_face`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face: Option<String>,
    /// What the face is (with `make_face`): `free_form` (a Free Form trimmed to the template) or
    /// `planar` (a planar patch of the template's region, when the kernel could not trim the Free
    /// Form).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub face_kind: Option<String>,
    /// The outline's sides in order, in mm: the dimensions between the points picked.
    pub edge_lengths: Vec<f64>,
    /// With `outline: "scan"`: the size of the grid's cells (mm), how true the outline is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// With `outline: "scan"` and `round`: the most a corner was moved by the tidying (mm).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved: Option<f64>,
    /// With `outline: "scan"`: the stretches of the outline with no scan near them (a tear bridged,
    /// a side drawn where the scan stops short), guessed rather than read: how many, and how long
    /// they are together (mm). Measure these by hand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guessed: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guessed_length: Option<f64>,
    /// With `panel`: what the straight panel came to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelMade>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}

/// Put a Free Form flat: every control point of it moved square onto one plane. One undo step.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanMakeFlatParams {
    /// The Free Form. Default: the selected one, else the only one.
    #[serde(default)]
    pub surface: Option<String>,
    /// The plane. Default: fitted to the Free Form's net, the points far off it left out, so a
    /// bump does not tilt it.
    #[serde(default)]
    pub plane: Option<TemplatePlane>,
}

/// What `scan.make_flat` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanMakeFlatResult {
    pub surface: String,
    pub faces: usize,
    /// The plane used: a point on it and its unit normal.
    pub point: [f64; 3],
    pub normal: [f64; 3],
    /// How far the net's points were from the plane, root mean square and most (mm).
    pub rms: f64,
    pub max: f64,
    /// The feature that keeps the surface.
    pub feature: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
