//! Section to Sketch (`scan.section_sketch`): where a plane cuts the scan, fitted with lines, arcs
//! and circles. With a sketch being edited the cut is that sketch's plane; else the dialog chooses
//! XY, XZ or YZ and how far along it. The fitted cut is drawn over the picture as it will be
//! sketched (lines in the sketch colour, arcs in the accent, the joints as dots); OK sketches it.

use serde_json::{Value, json};
use solidcore_record::{Plane, Vec2};
use solidcraft_engine::cmd::scan::section::{self, Outline, Piece};
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit::{self, choice_row};

/// The planes the dialog offers.
const PLANES: [&str; 3] = ["XY", "XZ", "YZ"];

/// Samples along each arc and circle drawn.
const ARC_SAMPLES: usize = 32;

/// The Section to Sketch dialog's values.
#[derive(Clone, Debug)]
pub struct SectionSketch {
    /// Which of [`PLANES`], and how far along its normal (mm).
    pub plane: usize,
    pub offset: f64,
    /// How far a piece may stray from the cut (mm).
    pub tolerance: f64,
    shown: Option<Shown>,
}

impl Default for SectionSketch {
    fn default() -> Self {
        SectionSketch { plane: 0, offset: 0.0, tolerance: section::TOLERANCE, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (usize, u64, u64, u64),
    result: Result<(String, Plane, Vec<Outline>), String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.section_sketch").then(|| (Kind::ScanSection(SectionSketch::default()), Vec::new()))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// Whether a sketch is being edited: the cut is its plane.
fn in_sketch(app: &SolveApp) -> bool {
    app.session.active_sketch.is_some()
}

/// Cut and fit again when the plane, the tolerance or the design has changed.
fn cut(app: &SolveApp, k: &mut SectionSketch) {
    let key = (k.plane, k.offset.to_bits(), k.tolerance.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let plane = section::standard_plane(PLANES[k.plane.min(PLANES.len() - 1)], k.offset);
    let result = plane
        .ok_or_else(|| "no such plane".to_owned())
        .and_then(|plane| section::preview(&app.session, None, plane, k.tolerance).map_err(|e| e.to_string()));
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut SectionSketch) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    if in_sketch(app) {
        ui_kit::text(ui, tr("ui.scan_section_in_sketch"), ui_kit::Text::Note("scan_section_in_sketch"));
    } else {
        choice_row(ui, tr("ui.scan_section_plane"), "scan_section_plane", &mut k.plane, &PLANES);
        form_row(ui, tr("ui.scan_section_offset"), |ui| ui_kit::length_field(ui, &mut k.offset, &units, decimals));
    }
    form_row(ui, tr("ui.scan_section_tolerance"), |ui| ui_kit::length_field(ui, &mut k.tolerance, &units, decimals.max(3)));
    k.tolerance = k.tolerance.clamp(1e-3, 10.0);
    cut(app, k);
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((_, _, outlines))) if outlines.is_empty() => {
            ui_kit::text(ui, tr("ui.scan_section_nothing"), ui_kit::Text::Report("scan_section_nothing", true));
        }
        Some(Ok((_, _, outlines))) => {
            let (lines, arcs, circles) = counts(outlines);
            let note = crate::i18n::format(
                "ui.scan_section_found",
                &[
                    ("outlines", &outlines.len().to_string()),
                    ("lines", &lines.to_string()),
                    ("arcs", &arcs.to_string()),
                    ("circles", &circles.to_string()),
                ],
            );
            ui_kit::text(ui, &note, ui_kit::Text::Note("scan_section_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_section_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_section_hint"), ui_kit::Text::Hint("scan_section_hint"));
    false
}

/// What OK runs: the cut sketched.
pub fn commands(app: &SolveApp, k: &SectionSketch) -> Result<Vec<(String, Value)>, String> {
    let (body, _, outlines) = k.shown.as_ref().ok_or("still cutting the scan")?.result.as_ref().map_err(Clone::clone)?;
    if outlines.is_empty() {
        return Err(tr("ui.scan_section_nothing").into());
    }
    let mut p = json!({"body": body, "tolerance": format!("{} mm", k.tolerance)});
    if !in_sketch(app) {
        p["plane"] = json!(PLANES[k.plane.min(PLANES.len() - 1)]);
        p["offset"] = json!(format!("{} mm", k.offset));
    }
    Ok(vec![("scan.section_sketch".into(), p)])
}

/// Points along a piece, in the plane.
fn along(piece: &Piece) -> Vec<Vec2> {
    match *piece {
        Piece::Line { from, to } => vec![from, to],
        Piece::Arc { center, radius, from, to, ccw } => {
            let (a, b) = ((from - center).angle(), (to - center).angle());
            let sweep = if ccw { (b - a).rem_euclid(std::f64::consts::TAU) } else { -(a - b).rem_euclid(std::f64::consts::TAU) };
            (0..=ARC_SAMPLES).map(|i| center + Vec2::from_angle(a + sweep * i as f64 / ARC_SAMPLES as f64) * radius).collect()
        }
    }
}

/// The fitted cut over the picture while the dialog is open.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanSection(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, plane, outlines))) = k.shown.as_ref().map(|s| &s.result) else { return };
    paint(painter, proj, plane, outlines);
}

/// Outlines fitted in `plane` drawn over the picture as they will be sketched: lines in the
/// sketch colour, arcs and circles in the accent, the joints as dots.
pub(crate) fn paint(painter: &egui::Painter, proj: &crate::viewport::Proj, plane: &Plane, outlines: &[Outline]) {
    let t = crate::theme::Tokens::get();
    let screen = |p: Vec2| proj.to_screen(plane.to_world(p));
    let line = |points: Vec<Vec2>, colour: egui::Color32| {
        let on: Vec<egui::Pos2> = points.into_iter().filter_map(screen).collect();
        painter.add(egui::Shape::line(on, egui::Stroke::new(2.0, colour)));
    };
    for outline in outlines {
        match outline {
            Outline::Circle { center, radius } => {
                line(
                    (0..=4 * ARC_SAMPLES)
                        .map(|i| *center + Vec2::from_angle(std::f64::consts::TAU * i as f64 / (4 * ARC_SAMPLES) as f64) * *radius)
                        .collect(),
                    t.accent,
                );
            }
            Outline::Chain(pieces) => {
                for piece in pieces {
                    line(along(piece), if matches!(piece, Piece::Arc { .. }) { t.accent } else { t.sketch_accent });
                    if let Some(p) = screen(piece.from()) {
                        painter.circle_filled(p, 2.5, t.text);
                    }
                }
            }
        }
    }
}

/// How many lines, arcs and circles the outlines are.
pub(crate) fn counts(outlines: &[Outline]) -> (usize, usize, usize) {
    let (mut lines, mut arcs, mut circles) = (0, 0, 0);
    for o in outlines {
        match o {
            Outline::Circle { .. } => circles += 1,
            Outline::Chain(pieces) => {
                for p in pieces {
                    match p {
                        Piece::Line { .. } => lines += 1,
                        Piece::Arc { .. } => arcs += 1,
                    }
                }
            }
        }
    }
    (lines, arcs, circles)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A slab with a round boss as an STL: cut through the boss the dialog shows one circle, and OK
    /// sketches it.
    #[test]
    fn the_cut_through_a_boss_is_one_circle_and_ok_sketches_it() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid b\n");
        let n = 96;
        let ring = |k: u32, z: f64| {
            let a = std::f64::consts::TAU * f64::from(k % n) / f64::from(n);
            [10.0 * a.cos(), 10.0 * a.sin(), z]
        };
        for k in 0..n {
            for tri in [[ring(k, 0.0), ring(k + 1, 0.0), ring(k + 1, 20.0)], [ring(k, 0.0), ring(k + 1, 20.0), ring(k, 20.0)]] {
                stl += "facet normal 0 0 0\nouter loop\n";
                for [x, y, z] in tri {
                    stl += &format!("vertex {x} {y} {z}\n");
                }
                stl += "endloop\nendfacet\n";
            }
        }
        let path = h.dir.join("tube.stl");
        std::fs::write(&path, stl + "endsolid b\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.section_sketch");
        if let Some(Kind::ScanSection(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.offset = 10.0;
        }
        h.frames(2);
        let Some(Kind::ScanSection(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog") };
        let Some(Ok((_, _, outlines))) = k.shown.as_ref().map(|s| &s.result) else { panic!("a cut: {:?}", k.shown) };
        assert!(matches!(outlines.as_slice(), [Outline::Circle { radius, .. }] if (radius - 10.0).abs() < 0.01), "{outlines:?}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == "Section"), "the sketch made");
    }
}
