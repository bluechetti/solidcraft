//! Export Panel (`scan.panel_export`): a straight panel's pieces for cutting, written from the
//! panel `scan.flat_template` would make with the same parameters: each piece at its own 0, 0 with
//! its outline, cut-outs, holes and fixing points, labelled and dimensioned (DXF); the blanks laid
//! on sheets (DXF); every hole and fixing point, the cut list and the distances to check in the
//! van before cutting (CSV). Nothing in the design changes.

use std::fmt::Write as _;

use serde_json::{Value, json};
use solidcore_record::Vec2;
use solidcraft_geom::prelude::*;
use solidcraft_io::{DxfEntity, DxfLayer, DxfShape, write_dxf_drawing};

use super::CommandSpec;
use super::template::{fixing_hole, hole_size, hole_sizes, panel, panel_keep_checked, prepare_panel};
use crate::params::str_;
use crate::schema::scan_template::{PanelCheckOut, PanelPieceOut, ScanPanelExportParams, ScanPanelExportResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanPanelExportParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.panel_export", "Export Panel", crate::checked!("scan.panel_export", export))
    .contract(&CONTRACT)
    .output(output_schema_for::<ScanPanelExportResult>)
    .params("the straight panel as for `scan.panel_proposals` (body?, plane?, tolerance?, side?, least_area?, gap?, step?, smooth?, round?, finger?, cover?, panel?: with what is dropped, moved and how each cut-out is cut); path?: the pieces' DXF (the sheets' DXF, holes CSV, cut list CSV and checks CSV are written beside it, named after it; without it their text is returned): each piece at its own 0, 0 (its blank's bottom-left corner seen from the room) with its outline, cut-outs, holes (layer HOLES, numbered), fixing points (FIXINGS, F1, F2, …), its number and blank size and dimension lines; the blanks laid on sheets; the design is not changed → files, pieces[{number, blank, sheet, holes, fixings}], sheets, checks[{from_piece, from_hole, to_piece, to_hole, distance, across, up}]")];

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
    for row in &rows {
        let tallest = row.iter().map(|q| q.blank[1]).fold(0.0, f64::max);
        y -= tallest;
        let mut x = 0.0f64;
        for piece in row {
            placed.push((piece, x, y));
            x += piece.blank[0] + SPACE;
        }
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
    let layers = [
        DxfLayer::new("PANEL", 7),
        DxfLayer::new("HOLES", 5),
        DxfLayer::new("FIXINGS", 3),
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
        files,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
