//! Export Panel (`scan.panel_export`): a straight panel's pieces for cutting, written from the
//! panel `scan.flat_template` would make with the same parameters: each piece at its own 0, 0 with
//! its outline, cut-outs, holes and fixing points, labelled and dimensioned (DXF); the blanks laid
//! on sheets (DXF); every hole and fixing point, the cut list and the distances to check in the
//! van before cutting (CSV); and the drawings (PDF: the whole panel with the lists, a page for
//! each piece with its corners and holes, the sheets). Nothing in the design changes.

use std::fmt::Write as _;

use serde_json::{Value, json};
use solidcore_record::Seg2;
use solidcore_record::Vec2;
use solidcraft_geom::paper::{Anchor, Group, Item, ObjectKind, Scene};
use solidcraft_geom::prelude::*;
use solidcraft_io::{DxfEntity, DxfLayer, DxfShape, write_dxf_drawing};

use super::CommandSpec;
use super::template::{fixing_hole, hole_size, hole_sizes, panel, panel_keep_checked, prepare_panel};
use crate::params::str_;
use crate::schema::scan_template::{PanelCheckOut, PanelInsertOut, PanelPieceOut, ScanPanelExportParams, ScanPanelExportResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanPanelExportParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.panel_export", "Export Panel", crate::checked!("scan.panel_export", export))
    .contract(&CONTRACT)
    .output(output_schema_for::<ScanPanelExportResult>)
    .params("the straight panel as for `scan.panel_proposals` (body?, plane?, tolerance?, side?, least_area?, gap?, step?, smooth?, round?, finger?, cover?, panel?: with what is dropped, moved and how each cut-out is cut); path?: the pieces' DXF (the sheets' DXF, holes CSV, cut list CSV, checks CSV and the drawings' PDF are written beside it, named after it; without it their text is returned, the PDF only counted): each piece at its own 0, 0 (its blank's bottom-left corner seen from the room) with its outline, cut-outs, holes (layer HOLES, numbered), fixing points (FIXINGS, F1, F2, …), its number and blank size and dimension lines; the blanks laid on sheets; the design is not changed → files, pieces[{number, blank, sheet, holes, fixings}], sheets, pdf_pages, checks[{from_piece, from_hole, to_piece, to_hole, distance, across, up}]")];

/// Room between pieces and sheets in the drawings (mm), and the text heights.
const SPACE: f64 = 300.0;
const TITLE: f64 = 40.0;
const LABEL: f64 = 8.0;

/// Pieces whose corners are within this of one another in height are on one row of the wall (mm).
const ROW_SPLIT: f64 = 100.0;

fn shape_line(points: &[Vec2], closed: bool) -> DxfShape {
    let mut points = points.to_vec();
    if closed && let Some(first) = points.first().copied() {
        points.push(first);
    }
    DxfShape::Polyline { points }
}

/// A dimension: the line `off` below (or left of) the stretch, ticks at its ends, the length.
fn dimension(out: &mut Vec<DxfEntity>, a: Vec2, b: Vec2, off: Vec2, text: String) {
    let layer = "DIMENSIONS".to_owned();
    let (pa, pb) = (a + off, b + off);
    let along = b - a;
    let angle = along.y.atan2(along.x).to_degrees();
    out.push(DxfEntity { layer: layer.clone(), shape: DxfShape::Line(pa, pb) });
    for (p, q) in [(a, pa), (b, pb)] {
        out.push(DxfEntity { layer: layer.clone(), shape: DxfShape::Line(p, q + off * 0.15) });
    }
    let mid = (pa + pb) * 0.5 + off * 0.35;
    out.push(DxfEntity { layer, shape: DxfShape::Text { at: mid, height: 20.0, angle_deg: angle, text } });
}

fn export(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.panel_export";
    let mut asked = p.clone();
    if let Some(o) = asked.as_object_mut() {
        o.remove("path");
        o.insert("outline".into(), json!("scan"));
        o.entry("panel").or_insert(json!({}));
    }
    let pp = prepare_panel(s, &asked, cmd)?;
    let keep = panel_keep_checked(&asked, &pp, cmd)?;
    let sizes = hole_sizes(&asked, cmd)?;
    let fixing = fixing_hole(&asked, cmd)?;
    let pieces = panel::pieces(&pp.proposal, &keep);
    if pieces.is_empty() {
        return Err(crate::params::bad(cmd, "the panel makes no pieces"));
    }
    // The pieces that drop into the openings behind the panel: each its own piece at its 0, 0.
    let inserts: Vec<Insert> = keep
        .inserts(&pp.proposal)
        .enumerate()
        .filter_map(|(k, (opening, found))| {
            let ring = found.piece(pp.proposal.clearance);
            let first = *ring.first()?;
            let (lo, hi) =
                ring.iter().fold((first, first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
            let fit = super::template::inserts::rounded_box(&ring);
            Some(Insert {
                piece: panel::Piece {
                    number: k + 1,
                    origin: lo,
                    outline: ring.iter().map(|q| *q - lo).collect(),
                    inside: Vec::new(),
                    points: Vec::new(),
                    blank: [hi.x - lo.x, hi.y - lo.y],
                },
                opening,
                depth: found.depth,
                radius: fit.map_or(0.0, |f| f.radius),
                off: fit.map_or(0.0, |f| f.off),
                at_wall: pp.wall(lo),
            })
        })
        .collect();
    let blanks: Vec<[f64; 2]> = pieces.iter().map(|q| q.blank).collect();
    let plans = panel::sheets(&blanks, pp.proposal.sheet, panel::KERF);
    let sheet_of = |n: usize| plans.iter().position(|plan| plan.pieces.iter().any(|pl| pl.piece + 1 == n)).map_or(0, |i| i + 1);
    let scanned = |q: Vec2| pp.proposal.holes.iter().find(|h| h.at.dist(q) < 1e-6).map_or(0.0, |h| h.diameter);
    // Whether the other scan has the hole too (with `confirm_with`).
    let seen = |q: Vec2| -> &'static str {
        let Some((_, _, seen)) = &pp.confirm else { return "" };
        pp.proposal.holes.iter().position(|h| h.at.dist(q) < 1e-6).and_then(|i| seen.get(i)).map_or("", |s| s.name())
    };

    // --- The pieces' drawing: each piece's row of the wall a row of the drawing, top row first.
    let mut ents: Vec<DxfEntity> = Vec::new();
    let mut holes_csv = String::from("piece,label,kind,x,y,drill,scanned,gap_behind_panel,seen\n");
    // The wall's rows (pieces whose corners are within a sloping seam's rise of one another),
    // each a row of the drawing below the one before, as tall as its tallest piece.
    let mut rows: Vec<Vec<&panel::Piece>> = Vec::new();
    for piece in &pieces {
        match rows.last_mut() {
            Some(row) if row.first().is_some_and(|f| (f.origin.y - piece.origin.y).abs() < ROW_SPLIT) => row.push(piece),
            _ => rows.push(vec![piece]),
        }
    }
    let mut placed: Vec<(&panel::Piece, f64, f64)> = Vec::new();
    let mut y = 0.0f64;
    let mut placed_width = 0.0f64;
    for row in &rows {
        let tallest = row.iter().map(|q| q.blank[1]).fold(0.0, f64::max);
        y -= tallest;
        let mut x = 0.0f64;
        for piece in row {
            placed.push((piece, x, y));
            x += piece.blank[0] + SPACE;
        }
        placed_width = placed_width.max(x - SPACE);
        y -= SPACE + 2.0 * TITLE;
    }
    for (piece, x, y) in placed {
        let at = |q: Vec2| Vec2::new(q.x + x, q.y + y);
        let [w, h] = piece.blank;
        ents.push(DxfEntity { layer: "PANEL".into(), shape: shape_line(&piece.outline.iter().map(|q| at(*q)).collect::<Vec<_>>(), true) });
        for r in &piece.inside {
            ents.push(DxfEntity { layer: "PANEL".into(), shape: shape_line(&r.iter().map(|q| at(*q)).collect::<Vec<_>>(), true) });
        }
        for pt in &piece.points {
            let (layer, d, label, kind) = match pt.kind {
                panel::PointKind::Hole => ("HOLES", hole_size(pt.diameter, &sizes), pt.number.to_string(), "hole"),
                panel::PointKind::Fixing => ("FIXINGS", fixing, format!("F{}", pt.number), "fixing"),
            };
            ents.push(DxfEntity { layer: layer.into(), shape: DxfShape::Circle { c: at(pt.at), r: d / 2.0 } });
            ents.push(DxfEntity {
                layer: "LABELS".into(),
                shape: DxfShape::Text { at: at(pt.at + Vec2::new(0.0, d / 2.0 + 4.0)), height: LABEL, angle_deg: 0.0, text: label.clone() },
            });
            let scan_d = if pt.kind == panel::PointKind::Hole { format!("{:.1}", scanned(pt.on_panel)) } else { String::new() };
            let seen_here = if pt.kind == panel::PointKind::Hole { seen(pt.on_panel) } else { "" };
            let _ = writeln!(holes_csv, "{},{label},{kind},{:.0},{:.0},{d:.1},{scan_d},{:.0},{seen_here}", piece.number, pt.at.x, pt.at.y, pt.behind);
        }
        ents.push(DxfEntity {
            layer: "LABELS".into(),
            shape: DxfShape::Text {
                at: at(Vec2::new(w / 2.0, h + TITLE)),
                height: TITLE,
                angle_deg: 0.0,
                text: format!("PIECE {}  {:.0} x {:.0}  (sheet {})", piece.number, w, h, sheet_of(piece.number)),
            },
        });
        dimension(&mut ents, at(Vec2::new(0.0, 0.0)), at(Vec2::new(w, 0.0)), Vec2::new(0.0, -60.0), format!("{w:.0}"));
        dimension(&mut ents, at(Vec2::new(0.0, h)), at(Vec2::new(0.0, 0.0)), Vec2::new(-60.0, 0.0), format!("{h:.0}"));
    }
    // The inserts in rows below the panel's pieces, as wide as the panel's widest row.
    let widest = placed_width.max(1000.0);
    let (mut x, mut row_h) = (0.0f64, 0.0f64);
    y -= SPACE;
    let mut at_of = Vec::new();
    for ins in &inserts {
        let [w, h] = ins.piece.blank;
        if x > 0.0 && x + w > widest {
            y -= row_h + SPACE + 2.0 * TITLE;
            (x, row_h) = (0.0, 0.0);
        }
        at_of.push((x, y - h));
        x += w + SPACE;
        row_h = row_h.max(h);
    }
    for (ins, (x, y)) in inserts.iter().zip(at_of) {
        let at = |q: Vec2| Vec2::new(q.x + x, q.y + y);
        let [w, h] = ins.piece.blank;
        ents.push(DxfEntity { layer: "INSERTS".into(), shape: shape_line(&ins.piece.outline.iter().map(|q| at(*q)).collect::<Vec<_>>(), true) });
        ents.push(DxfEntity {
            layer: "LABELS".into(),
            shape: DxfShape::Text { at: at(Vec2::new(w / 2.0, h + TITLE)), height: TITLE, angle_deg: 0.0, text: ins.title() },
        });
        dimension(&mut ents, at(Vec2::new(0.0, 0.0)), at(Vec2::new(w, 0.0)), Vec2::new(0.0, -60.0), format!("{w:.0}"));
        dimension(&mut ents, at(Vec2::new(0.0, h)), at(Vec2::new(0.0, 0.0)), Vec2::new(-60.0, 0.0), format!("{h:.0}"));
    }
    let mut inserts_csv = String::from("insert,opening,width,height,corner_radius,off_rounded_box,depth,x_on_wall,y_on_wall\n");
    for ins in &inserts {
        let _ = writeln!(
            inserts_csv,
            "{},{},{:.0},{:.0},{:.0},{:.1},{:.0},{:.0},{:.0}",
            ins.piece.number, ins.opening, ins.piece.blank[0], ins.piece.blank[1], ins.radius, ins.off, ins.depth, ins.at_wall[0], ins.at_wall[1]
        );
    }
    let layers = [
        DxfLayer::new("PANEL", 7),
        DxfLayer::new("HOLES", 5),
        DxfLayer::new("FIXINGS", 3),
        DxfLayer::new("INSERTS", 6),
        DxfLayer::new("LABELS", 2),
        DxfLayer::new("DIMENSIONS", 8),
    ];
    let pieces_dxf = write_dxf_drawing(&layers, &ents)?;

    // --- The sheets: each sheet's outline and the blanks on it, side by side.
    let sheet = pp.proposal.sheet;
    let (long, short) = (sheet[0].max(sheet[1]), sheet[0].min(sheet[1]));
    let mut sheet_ents: Vec<DxfEntity> = Vec::new();
    let mut cut_list = String::from("piece,width,height,sheet,x_on_sheet,y_on_sheet,turned,holes,fixings\n");
    for (i, plan) in plans.iter().enumerate() {
        let ox = i as f64 * (long + SPACE);
        let corner = |x: f64, y: f64, w: f64, h: f64| {
            vec![Vec2::new(ox + x, y), Vec2::new(ox + x + w, y), Vec2::new(ox + x + w, y + h), Vec2::new(ox + x, y + h)]
        };
        sheet_ents.push(DxfEntity { layer: "SHEETS".into(), shape: shape_line(&corner(0.0, 0.0, long, short), true) });
        sheet_ents.push(DxfEntity {
            layer: "LABELS".into(),
            shape: DxfShape::Text {
                at: Vec2::new(ox + long / 2.0, short + TITLE),
                height: TITLE,
                angle_deg: 0.0,
                text: format!("SHEET {}{}", i + 1, if plan.too_big { "  (PIECE TOO BIG FOR THE SHEET)" } else { "" }),
            },
        });
        for pl in &plan.pieces {
            sheet_ents.push(DxfEntity { layer: "PANEL".into(), shape: shape_line(&corner(pl.at.x, pl.at.y, pl.size[0], pl.size[1]), true) });
            sheet_ents.push(DxfEntity {
                layer: "LABELS".into(),
                shape: DxfShape::Text {
                    at: Vec2::new(ox + pl.at.x + pl.size[0] / 2.0, pl.at.y + pl.size[1] / 2.0),
                    height: TITLE,
                    angle_deg: 0.0,
                    text: format!("{}  {:.0} x {:.0}", pl.piece + 1, pl.size[0], pl.size[1]),
                },
            });
        }
    }
    for piece in &pieces {
        let placed = plans.iter().flat_map(|plan| &plan.pieces).find(|pl| pl.piece + 1 == piece.number);
        let count = |kind| piece.points.iter().filter(|q| q.kind == kind).count();
        let _ = writeln!(
            cut_list,
            "{},{:.0},{:.0},{},{:.0},{:.0},{},{},{}",
            piece.number,
            piece.blank[0],
            piece.blank[1],
            sheet_of(piece.number),
            placed.map_or(0.0, |pl| pl.at.x),
            placed.map_or(0.0, |pl| pl.at.y),
            placed.is_some_and(|pl| pl.turned),
            count(panel::PointKind::Hole),
            count(panel::PointKind::Fixing)
        );
    }
    let sheets_dxf = write_dxf_drawing(&[DxfLayer::new("SHEETS", 7), DxfLayer::new("PANEL", 1), DxfLayer::new("LABELS", 2)], &sheet_ents)?;

    // --- Distances to check in the van.
    let checks = panel::checks(&pieces);

    // --- The drawings as a PDF: the panel, a page for each piece, the sheets.
    let confirm_line = pp.confirm.as_ref().map(|(body, fit, seen)| match fit {
        Some(f) => {
            let count = |w: panel::Seen| seen.iter().filter(|s| **s == w).count();
            format!(
                "Holes checked against {body}{}: {} in both, {} not found there, {} not covered by it (lined up within {:.1} mm).",
                if f.mirror { " (the wall opposite)" } else { "" },
                count(panel::Seen::Both),
                count(panel::Seen::NotFound),
                count(panel::Seen::Unscanned),
                f.rms
            )
        }
        None => format!("Holes could not be lined up with {body}."),
    });
    let seen_of = |q: Vec2| seen(q);
    let pages =
        drawings(&pieces, &inserts, &plans, &checks, pp.proposal.sheet, &sizes, fixing, pp.proposal.clearance, confirm_line.as_deref(), &seen_of);
    let prepared: Vec<_> =
        pages.iter().map(|page| page.prepare()).collect::<std::result::Result<_, _>>().map_err(|e| EngineError::Other(e.to_string()))?;
    let pdf = solidcraft_io::paper::paper_pdf(&prepared)?;
    let mut checks_csv = String::from("from_piece,from_hole,to_piece,to_hole,distance,across,up\n");
    for c in &checks {
        let _ = writeln!(checks_csv, "{},{},{},{},{:.0},{:.0},{:.0}", c.from.0, c.from.1, c.to.0, c.to.1, c.distance, c.across, c.up);
    }

    let mut files = Vec::new();
    let path = str_(p, "path").filter(|x| !x.trim().is_empty());
    if let Some(path) = path {
        let stem = path.strip_suffix(".dxf").or_else(|| path.strip_suffix(".DXF")).unwrap_or(path);
        let write = |name: String, text: &str, files: &mut Vec<String>| -> Result<()> {
            solidcraft_io::vfs::write(&name, text.as_bytes()).map_err(|e| EngineError::Other(format!("{name}: {e}")))?;
            files.push(name);
            Ok(())
        };
        write(format!("{stem}.dxf"), &pieces_dxf, &mut files)?;
        write(format!("{stem}-sheets.dxf"), &sheets_dxf, &mut files)?;
        write(format!("{stem}-holes.csv"), &holes_csv, &mut files)?;
        write(format!("{stem}-cut-list.csv"), &cut_list, &mut files)?;
        write(format!("{stem}-checks.csv"), &checks_csv, &mut files)?;
        if !inserts.is_empty() {
            write(format!("{stem}-inserts.csv"), &inserts_csv, &mut files)?;
        }
        let name = format!("{stem}.pdf");
        solidcraft_io::vfs::write(&name, &pdf).map_err(|e| EngineError::Other(format!("{name}: {e}")))?;
        files.push(name);
    }
    let reply = ScanPanelExportResult {
        pieces: pieces
            .iter()
            .map(|q| PanelPieceOut {
                number: q.number,
                blank: q.blank,
                sheet: sheet_of(q.number),
                holes: q.points.iter().filter(|x| x.kind == panel::PointKind::Hole).count(),
                fixings: q.points.iter().filter(|x| x.kind == panel::PointKind::Fixing).count(),
            })
            .collect(),
        sheets: plans.len(),
        pdf_pages: pages.len(),
        inserts: inserts
            .iter()
            .map(|i| PanelInsertOut {
                number: i.piece.number,
                opening: i.opening,
                size: i.piece.blank,
                depth: i.depth,
                corner_radius: i.radius,
                off_rounded_box: i.off,
            })
            .collect(),
        checks: checks
            .iter()
            .map(|c| PanelCheckOut {
                from_piece: c.from.0,
                from_hole: c.from.1,
                to_piece: c.to.0,
                to_hole: c.to.1,
                distance: c.distance,
                across: c.across,
                up: c.up,
            })
            .collect(),
        pieces_dxf: path.is_none().then_some(pieces_dxf),
        sheets_dxf: path.is_none().then_some(sheets_dxf),
        holes_csv: path.is_none().then_some(holes_csv),
        cut_list_csv: path.is_none().then_some(cut_list),
        checks_csv: path.is_none().then_some(checks_csv),
        inserts_csv: path.is_none().then_some(inserts_csv),
        files,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// A piece cut to drop into an opening behind the panel: drawn as a piece of its own (`origin` where
/// its box's corner is on the panel), the opening's number, how deep it is and the rounded box
/// nearest it (corner radius, the farthest it strays from it), and its corner on the wall.
struct Insert {
    piece: panel::Piece,
    opening: usize,
    depth: f64,
    radius: f64,
    off: f64,
    at_wall: [f64; 2],
}

impl Insert {
    fn title(&self) -> String {
        let [w, h] = self.piece.blank;
        format!("INSERT {}  {w:.0} x {h:.0}  (depth {:.0})", self.piece.number, self.depth)
    }
    /// How to cut it: as a rounded box when it strays little from one, else along its trace.
    fn how(&self) -> String {
        let [w, h] = self.piece.blank;
        if self.off <= ROUNDED_OFF {
            format!(
                "Cut as a {w:.0} x {h:.0} box with corners of about {:.0} mm radius (the trace is within {:.0} mm of it).",
                self.radius,
                self.off.max(1.0)
            )
        } else {
            format!("Cut along the trace: it is up to {:.0} mm off a {w:.0} x {h:.0} box with rounded corners.", self.off)
        }
    }
}

/// A trace this close to a rounded box (mm) can be cut as that box.
const ROUNDED_OFF: f64 = 6.0;

/// Letter, landscape (mm), and the margin kept clear.
const PAGE: [f64; 2] = [279.4, 215.9];
const MARGIN: f64 = 12.0;

fn line_item(points: &[Vec2], closed: bool, width_mm: f64) -> Option<Item> {
    let mut segs: Vec<Seg2> = points.windows(2).map(|w| Seg2::Line { a: w[0], b: w[1] }).collect();
    if closed && let (Some(a), Some(b)) = (points.last(), points.first()) {
        segs.push(Seg2::Line { a: *a, b: *b });
    }
    segs.retain(|s| match s {
        Seg2::Line { a, b } => a.dist(*b) > 1e-6,
        _ => true,
    });
    (!segs.is_empty()).then_some(Item::Stroke { path: segs, closed, width_mm })
}

fn circle_item(c: Vec2, r: f64, width_mm: f64) -> Item {
    Item::Stroke { path: vec![Seg2::Arc { center: c, radius: r, start: 0.0, sweep: std::f64::consts::TAU }], closed: true, width_mm }
}

fn text_item(at: Vec2, height_mm: f64, anchor: Anchor, text: impl Into<String>) -> Item {
    let mut text: String = text.into();
    text.retain(|c| !c.is_control());
    text.truncate(400);
    if text.is_empty() {
        text.push(' ');
    }
    Item::Text { at, height_mm, angle: 0.0, anchor, text }
}

/// How a drawing fits a box on the page: its points scaled by `scale` and moved by `shift`.
struct Fit {
    scale: f64,
    shift: Vec2,
}

impl Fit {
    /// The fit of the box `lo`–`hi` into the page box `at`–`to`, centred.
    fn of(lo: Vec2, hi: Vec2, at: Vec2, to: Vec2) -> Fit {
        let size = hi - lo;
        let room = to - at;
        let scale = (room.x / size.x.max(1e-9)).min(room.y / size.y.max(1e-9));
        let used = size * scale;
        Fit { scale, shift: at + (room - used) * 0.5 - lo * scale }
    }
    fn at(&self, q: Vec2) -> Vec2 {
        q * self.scale + self.shift
    }
}

/// One page's items, as one group of each kind.
fn page(view: Vec<Item>, notes: Vec<Item>) -> Scene {
    Scene {
        size_mm: Vec2::new(PAGE[0], PAGE[1]),
        groups: vec![Group { object: 1, kind: ObjectKind::View, items: view }, Group { object: 2, kind: ObjectKind::Annotation, items: notes }],
    }
}

/// The drawing pages: the whole panel with the lists, a page for each piece, the sheets.
#[allow(clippy::too_many_arguments)]
fn drawings(
    pieces: &[panel::Piece],
    inserts: &[Insert],
    plans: &[panel::SheetPlan],
    checks: &[panel::Check],
    sheet: [f64; 2],
    sizes: &[f64],
    fixing: f64,
    clearance: f64,
    confirm: Option<&str>,
    seen: &dyn Fn(Vec2) -> &'static str,
) -> Vec<Scene> {
    let mut pages = Vec::new();
    let sheet_of = |n: usize| plans.iter().position(|plan| plan.pieces.iter().any(|pl| pl.piece + 1 == n)).map_or(0, |i| i + 1);
    let (long, short) = (sheet[0].max(sheet[1]), sheet[0].min(sheet[1]));
    let top = PAGE[1] - MARGIN;

    // --- The whole panel.
    let all: Vec<Vec2> = pieces.iter().flat_map(|p| p.outline.iter().map(move |q| *q + p.origin)).collect();
    if let Some(first) = all.first().copied() {
        let (lo, hi) =
            all.iter().fold((first, first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
        let fit = Fit::of(lo, hi, Vec2::new(MARGIN, 96.0), Vec2::new(PAGE[0] - MARGIN, top - 16.0));
        let (mut view, mut notes) = (Vec::new(), Vec::new());
        notes.push(text_item(
            Vec2::new(MARGIN, top - 4.0),
            6.0,
            Anchor::Start,
            format!("Wall panel: {} pieces from {} sheets of {long:.0} x {short:.0}", pieces.len(), plans.len()),
        ));
        notes.push(text_item(
            Vec2::new(MARGIN, top - 10.0),
            2.8,
            Anchor::Start,
            format!("Seen from the room. mm. A {clearance:.0} mm gap is left at every edge, round every cut-out and at every seam. Circles: holes in the frame; squares: new fixing points."),
        ));
        for p in pieces {
            let ring: Vec<Vec2> = p.outline.iter().map(|q| fit.at(*q + p.origin)).collect();
            view.extend(line_item(&ring, true, 0.3));
            for r in &p.inside {
                view.extend(line_item(&r.iter().map(|q| fit.at(*q + p.origin)).collect::<Vec<_>>(), true, 0.25));
            }
            for pt in &p.points {
                let c = fit.at(pt.on_panel);
                match pt.kind {
                    panel::PointKind::Hole => view.push(circle_item(c, 0.7, 0.15)),
                    panel::PointKind::Fixing => {
                        let h = 0.7;
                        view.extend(line_item(&[c + Vec2::new(-h, -h), c + Vec2::new(h, -h), c + Vec2::new(h, h), c + Vec2::new(-h, h)], true, 0.2));
                    }
                }
            }
            let mid = ring.iter().fold(Vec2::new(0.0, 0.0), |s, q| s + *q) / ring.len().max(1) as f64;
            notes.push(text_item(mid - Vec2::new(0.0, 3.0), 8.0, Anchor::Middle, p.number.to_string()));
        }
        // The openings behind, each with its insert's number.
        for i in inserts {
            let ring: Vec<Vec2> = i.piece.outline.iter().map(|q| fit.at(*q + i.piece.origin)).collect();
            view.extend(line_item(&ring, true, 0.12));
            let [w, h] = i.piece.blank;
            // A quarter of the way along, clear of the piece's number in the middle.
            let at = fit.at(i.piece.origin + Vec2::new(w / 4.0, h / 2.0));
            notes.push(text_item(at, 2.2, Anchor::Middle, format!("I{}", i.piece.number)));
        }
        let mut y = 84.0;
        notes.push(text_item(Vec2::new(MARGIN, y), 3.5, Anchor::Start, "Pieces"));
        for p in pieces {
            y -= 4.4;
            let holes = p.points.iter().filter(|q| q.kind == panel::PointKind::Hole).count();
            let fixings = p.points.len() - holes;
            let fits = (p.blank[0] <= long + 0.5 && p.blank[1] <= short + 0.5) || (p.blank[0] <= short + 0.5 && p.blank[1] <= long + 0.5);
            notes.push(text_item(
                Vec2::new(MARGIN, y),
                2.6,
                Anchor::Start,
                format!(
                    "{}:  {:.0} x {:.0} mm,  sheet {},  {holes} holes,  {fixings} fixing points{}",
                    p.number,
                    p.blank[0],
                    p.blank[1],
                    sheet_of(p.number),
                    if fits { "" } else { "  - LONGER THAN THE SHEET" }
                ),
            ));
        }
        let mut y = 84.0;
        let x = 145.0;
        notes.push(text_item(Vec2::new(x, y), 3.5, Anchor::Start, "Measure in the van before cutting"));
        for c in checks {
            y -= 4.4;
            notes.push(text_item(
                Vec2::new(x, y),
                2.6,
                Anchor::Start,
                format!(
                    "Piece {} hole {} to piece {} hole {}:  {:.0} mm  (across {:.0}, up {:.0})",
                    c.from.0, c.from.1, c.to.0, c.to.1, c.distance, c.across, c.up
                ),
            ));
        }
        if let Some(line) = confirm {
            y -= 8.0;
            for (k, part) in wrap(line, 70).into_iter().enumerate() {
                notes.push(text_item(Vec2::new(x, y - 4.0 * k as f64), 2.6, Anchor::Start, part));
            }
        }
        pages.push(page(view, notes));
    }

    // --- A page for each piece.
    for p in pieces {
        let [w, h] = p.blank;
        let note = format!(
            "Blank {w:.0} x {h:.0} mm from sheet {}. 0,0 is the blank's bottom-left corner seen from the room; x along the bottom, y straight up. Gaps already taken off.",
            sheet_of(p.number)
        );
        pages.push(piece_page(p, format!("Piece {} of {}", p.number, pieces.len()), &note, sizes, fixing, seen));
    }

    // --- The inserts: all of them, then a page for each.
    if !inserts.is_empty() {
        let (mut view, mut notes) = (Vec::new(), Vec::new());
        notes.push(text_item(
            Vec2::new(MARGIN, top - 4.0),
            6.0,
            Anchor::Start,
            format!("Inserts: {} pieces for the openings behind the panel", inserts.len()),
        ));
        notes.push(text_item(
            Vec2::new(MARGIN, top - 10.0),
            2.8,
            Anchor::Start,
            format!("Traced from the scan at each opening's lip, {clearance:.0} mm taken off all round so they drop in. Depth: how far the opening goes back behind the panel."),
        ));
        let cols = inserts.len().clamp(1, 4);
        let rows_n = inserts.len().div_ceil(cols).max(1);
        let list_h = 6.0 + 4.0 * inserts.len().min(14) as f64;
        let cell_w = (PAGE[0] - 2.0 * MARGIN) / cols as f64;
        let cell_h = ((top - 16.0 - MARGIN - list_h) / rows_n as f64).max(10.0);
        // One scale for all, so their sizes compare.
        let biggest = inserts.iter().map(|i| (i.piece.blank[0] / (cell_w - 8.0)).max(i.piece.blank[1] / (cell_h - 10.0))).fold(1e-9, f64::max);
        for (k, i) in inserts.iter().enumerate() {
            let (col, row) = (k % cols, k / cols);
            let corner = Vec2::new(MARGIN + col as f64 * cell_w + 4.0, top - 16.0 - (row + 1) as f64 * cell_h + 6.0);
            let ring: Vec<Vec2> = i.piece.outline.iter().map(|q| corner + *q / biggest).collect();
            view.extend(line_item(&ring, true, 0.25));
            let [w, h] = i.piece.blank;
            notes.push(text_item(
                corner + Vec2::new(w / biggest / 2.0, h / biggest + 1.5),
                2.6,
                Anchor::Middle,
                format!("I{}  {w:.0} x {h:.0}", i.piece.number),
            ));
        }
        let mut y = MARGIN + list_h - 4.0;
        for i in inserts.iter().take(14) {
            notes.push(text_item(Vec2::new(MARGIN, y), 2.4, Anchor::Start, format!("I{}:  depth {:.0} mm.  {}", i.piece.number, i.depth, i.how())));
            y -= 4.0;
        }
        pages.push(page(view, notes));
        for i in inserts {
            let note = format!(
                "{} Depth {:.0} mm. 0,0 is the box's bottom-left corner seen from the room; on the wall it is at {:.0}, {:.0} from the panel's corner.",
                i.how(),
                i.depth,
                i.at_wall[0],
                i.at_wall[1]
            );
            pages.push(piece_page(&i.piece, format!("Insert {} of {}", i.piece.number, inserts.len()), &note, sizes, fixing, seen));
        }
    }

    // --- The sheets.
    let (mut view, mut notes) = (Vec::new(), Vec::new());
    notes.push(text_item(Vec2::new(MARGIN, top - 4.0), 6.0, Anchor::Start, format!("Cutting: {} sheets of {long:.0} x {short:.0}", plans.len())));
    notes.push(text_item(
        Vec2::new(MARGIN, top - 10.0),
        2.8,
        Anchor::Start,
        "Each piece's blank on its sheet, in rows a saw cuts straight across (3 mm kerf allowed).",
    ));
    let cols = 2usize;
    let rows_n = plans.len().div_ceil(cols).max(1);
    let cell_w = (PAGE[0] - 2.0 * MARGIN) / cols as f64;
    let cell_h = (top - 18.0 - MARGIN) / rows_n as f64;
    for (i, plan) in plans.iter().enumerate() {
        let (col, row) = (i % cols, i / cols);
        let at = Vec2::new(MARGIN + col as f64 * cell_w + 4.0, top - 18.0 - (row + 1) as f64 * cell_h + 8.0);
        let to = at + Vec2::new(cell_w - 8.0, cell_h - 16.0);
        let fit = Fit::of(Vec2::new(0.0, 0.0), Vec2::new(long, short), at, to);
        let rect = |x: f64, y: f64, w: f64, h: f64| {
            vec![fit.at(Vec2::new(x, y)), fit.at(Vec2::new(x + w, y)), fit.at(Vec2::new(x + w, y + h)), fit.at(Vec2::new(x, y + h))]
        };
        view.extend(line_item(&rect(0.0, 0.0, long, short), true, 0.35));
        notes.push(text_item(
            fit.at(Vec2::new(long / 2.0, short)) + Vec2::new(0.0, 2.0),
            3.0,
            Anchor::Middle,
            format!("Sheet {}{}", i + 1, if plan.too_big { " (piece too big for the sheet)" } else { "" }),
        ));
        for pl in &plan.pieces {
            view.extend(line_item(&rect(pl.at.x, pl.at.y, pl.size[0], pl.size[1]), true, 0.25));
            notes.push(text_item(
                fit.at(Vec2::new(pl.at.x + pl.size[0] / 2.0, pl.at.y + pl.size[1] / 2.0)) - Vec2::new(0.0, 1.0),
                2.6,
                Anchor::Middle,
                format!("{}  {:.0} x {:.0}{}", pl.piece + 1, pl.size[0], pl.size[1], if pl.turned { " (turned)" } else { "" }),
            ));
        }
    }
    pages.push(page(view, notes));
    pages
}

/// A piece's page: `title` and `note` over it, its outline to scale with its corners lettered (or
/// numbered when it has many) and its blank's size, the corners and its holes as tables.
fn piece_page(p: &panel::Piece, title: String, note: &str, sizes: &[f64], fixing: f64, seen: &dyn Fn(Vec2) -> &'static str) -> Scene {
    let top = PAGE[1] - MARGIN;
    let (mut view, mut notes) = (Vec::new(), Vec::new());
    let [w, h] = p.blank;
    let lo = p.outline.iter().fold(Vec2::new(0.0, 0.0), |m, q| Vec2::new(m.x.min(q.x), m.y.min(q.y)));
    let fit = Fit::of(lo, Vec2::new(w.max(1.0), h.max(1.0)), Vec2::new(MARGIN + 12.0, 104.0), Vec2::new(PAGE[0] - MARGIN - 12.0, top - 22.0));
    notes.push(text_item(Vec2::new(MARGIN, top - 4.0), 6.0, Anchor::Start, title));
    for (k, line) in wrap(note, 150).into_iter().enumerate() {
        notes.push(text_item(Vec2::new(MARGIN, top - 10.0 - 3.6 * k as f64), 2.8, Anchor::Start, line));
    }
    let ring: Vec<Vec2> = p.outline.iter().map(|q| fit.at(*q)).collect();
    view.extend(line_item(&ring, true, 0.35));
    for r in &p.inside {
        view.extend(line_item(&r.iter().map(|q| fit.at(*q)).collect::<Vec<_>>(), true, 0.3));
    }
    // Corners lettered, out from the middle.
    let letters: Vec<char> = ('A'..='Z').chain('a'..='z').collect();
    let numbered = p.outline.len() > letters.len();
    let every = if numbered { p.outline.len().div_ceil(40) } else { 1 };
    let corner = |k: usize| if numbered { (k + 1).to_string() } else { letters.get(k % letters.len()).map_or(String::new(), |c| c.to_string()) };
    let mid = ring.iter().fold(Vec2::new(0.0, 0.0), |s, q| s + *q) / ring.len().max(1) as f64;
    for (k, q) in ring.iter().enumerate() {
        let out = *q - mid;
        let len = (out.x * out.x + out.y * out.y).sqrt().max(1e-9);
        let at = *q + out * (2.6 / len) - Vec2::new(0.0, 0.8);
        // A long trace's corners labelled every few, so the labels stay apart.
        if k % every == 0 {
            notes.push(text_item(at, if numbered { 1.6 } else { 2.0 }, Anchor::Middle, corner(k)));
        }
    }
    // Holes and fixing points, numbered.
    for pt in &p.points {
        let c = fit.at(pt.at);
        let (r, label) = match pt.kind {
            panel::PointKind::Hole => (hole_size(pt.diameter, sizes) / 2.0, pt.number.to_string()),
            panel::PointKind::Fixing => (fixing / 2.0, format!("F{}", pt.number)),
        };
        let rr = (r * fit.scale).max(0.6);
        view.push(circle_item(c, rr, if pt.kind == panel::PointKind::Fixing { 0.3 } else { 0.18 }));
        notes.push(text_item(c + Vec2::new(0.0, rr + 0.6), 1.7, Anchor::Middle, label));
    }
    // The blank's width below it and height to its left.
    let (a, b, c) = (fit.at(Vec2::new(0.0, 0.0)), fit.at(Vec2::new(w, 0.0)), fit.at(Vec2::new(0.0, h)));
    let below = a.y.min(ring.iter().map(|q| q.y).fold(f64::MAX, f64::min)) - 6.0;
    view.extend(line_item(&[Vec2::new(a.x, below), Vec2::new(b.x, below)], false, 0.2));
    for x in [a.x, b.x] {
        view.extend(line_item(&[Vec2::new(x, below - 1.5), Vec2::new(x, below + 1.5)], false, 0.2));
    }
    notes.push(text_item(Vec2::new((a.x + b.x) / 2.0, below - 4.0), 3.0, Anchor::Middle, format!("{w:.0}")));
    let left = a.x - 7.0;
    view.extend(line_item(&[Vec2::new(left, a.y), Vec2::new(left, c.y)], false, 0.2));
    for y in [a.y, c.y] {
        view.extend(line_item(&[Vec2::new(left - 1.5, y), Vec2::new(left + 1.5, y)], false, 0.2));
    }
    notes.push(Item::Text {
        at: Vec2::new(left - 1.8, (a.y + c.y) / 2.0),
        height_mm: 3.0,
        angle: std::f64::consts::FRAC_PI_2,
        anchor: Anchor::Middle,
        text: format!("{h:.0}"),
    });
    // The corners and the holes as tables.
    let pitch = 3.3;
    let first_row = 86.0;
    let rows = ((first_row - MARGIN) / pitch).floor() as usize;
    notes.push(text_item(Vec2::new(MARGIN, first_row + 4.0), 3.2, Anchor::Start, "Corners (x, y mm)"));
    let corner_cols = p.outline.len().div_ceil(rows.max(1));
    for (k, q) in p.outline.iter().enumerate() {
        let (col, row) = (k / rows.max(1), k % rows.max(1));
        let label = corner(k);
        notes.push(text_item(
            Vec2::new(MARGIN + col as f64 * 26.0, first_row - row as f64 * pitch),
            2.3,
            Anchor::Start,
            format!("{label}  {:.0}, {:.0}", q.x, q.y),
        ));
    }
    let table_x = MARGIN + corner_cols as f64 * 26.0 + 6.0;
    if p.points.is_empty() {
        return page(view, notes);
    }
    notes.push(text_item(Vec2::new(table_x, first_row + 4.0), 3.2, Anchor::Start, "Holes and fixing points (F): x, y, drill, gap behind"));
    let col_w = 64.0;
    for (k, pt) in p.points.iter().enumerate() {
        let (col, row) = (k / rows.max(1), k % rows.max(1));
        let x = table_x + col as f64 * col_w;
        if x + col_w > PAGE[0] - MARGIN + 20.0 {
            notes.push(text_item(Vec2::new(table_x, MARGIN - 6.0), 2.3, Anchor::Start, "More holes in the holes list (CSV)."));
            break;
        }
        let (label, d) = match pt.kind {
            panel::PointKind::Hole => (pt.number.to_string(), hole_size(pt.diameter, sizes)),
            panel::PointKind::Fixing => (format!("F{}", pt.number), fixing),
        };
        let checked = if pt.kind == panel::PointKind::Hole {
            match seen(pt.on_panel) {
                "not_found" => "  not in other scan",
                "unscanned" => "  not checked",
                _ => "",
            }
        } else {
            ""
        };
        notes.push(text_item(
            Vec2::new(x, first_row - row as f64 * pitch),
            2.3,
            Anchor::Start,
            format!("{label}   {:.0}, {:.0}   d {d:.1}   gap {:.0}{checked}", pt.at.x, pt.at.y, pt.behind),
        ));
    }
    page(view, notes)
}

/// `text` in lines of at most `width` characters, broken at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match out.last_mut() {
            Some(line) if line.len() + 1 + word.len() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => out.push(word.to_owned()),
        }
    }
    out
}
