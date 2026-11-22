//! Sections Along Curve (`scan.sections_along_curve`): the scan cut by planes square to a 3D curve
//! at even steps along it, each section fitted with lines, arcs and circles as Section to Sketch
//! fits a cut. The curve is the last one drawn with Curve on Scan until another 3D curve is clicked.
//! The sections are drawn over the picture as they will be sketched; OK keeps each as a sketch on
//! its plane, in order along the curve, ready to loft.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::cmd::scan::section::{self, Along, Outline};

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::ui_kit;

/// The Sections Along Curve dialog's values.
#[derive(Clone, Debug)]
pub struct SectionsAlong {
    /// The curve clicked: its 3D sketch and id. None: the last curve of `Scan Curves`.
    path: Option<(u64, String)>,
    pub count: usize,
    /// Every loop each plane cuts, not just the one round the curve.
    pub every: bool,
    /// How far a piece may stray from the cut (mm).
    pub tolerance: f64,
    /// The tolerance follows the cuts' noise until one is typed (Section to Sketch's preference).
    pub from_scan: bool,
    shown: Option<Shown>,
}

impl Default for SectionsAlong {
    fn default() -> Self {
        SectionsAlong { path: None, count: section::SECTIONS, every: false, tolerance: section::TOLERANCE, from_scan: false, shown: None }
    }
}

type Cut = (String, Vec<Vec3>, Vec<(Along, Vec<Outline>)>);

#[derive(Clone, Debug)]
struct Shown {
    key: (Option<(u64, String)>, usize, bool, u64, u64, bool),
    result: Result<Cut, String>,
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    let from_scan = app.preferences.section_tolerance_from_scan;
    (id == "scan.sections_along_curve").then(|| (Kind::ScanSections(SectionsAlong { from_scan, ..SectionsAlong::default() }), Vec::new()))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// Cut and fit again when the curve, the count, the loops kept, the tolerance or the design has
/// changed.
fn cut(app: &SolveApp, k: &mut SectionsAlong) {
    let tolerance = if k.from_scan { 0 } else { k.tolerance.to_bits() };
    let key = (k.path.clone(), k.count, k.every, tolerance, app.session.revision, k.from_scan);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let (sketch, curves) = match &k.path {
        Some((sketch, id)) => (Some(*sketch), vec![id.clone()]),
        None => (None, Vec::new()),
    };
    let asked = (!k.from_scan).then_some(k.tolerance);
    let result = section::along_preview(&app.session, None, sketch, &curves, k.count, k.every, asked).map_err(|e| e.to_string());
    let result = result.map(|(body, path, sections, used)| {
        if k.from_scan {
            k.tolerance = used;
        }
        (body, path, sections)
    });
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut SectionsAlong) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let path = match &k.path {
        Some((sketch, id)) => {
            let name = app.session.doc.feature(*sketch).map_or_else(String::new, |f| f.name.clone());
            format!("{name} · {id}")
        }
        None => tr("ui.scan_sections_last_curve").to_owned(),
    };
    form_row(ui, tr("ui.scan_sections_path"), |ui| ui_kit::text(ui, &path, ui_kit::Text::Cell));
    form_row(ui, tr("ui.scan_sections_count"), |ui| ui_kit::number_field(ui, &mut k.count, 1..=60, 0.2, ""));
    k.count = k.count.clamp(1, 60);
    form_row(ui, tr("ui.scan_sections_every"), |ui| check(ui, &mut k.every));
    let before = k.tolerance;
    form_row(ui, tr("ui.scan_section_tolerance"), |ui| ui_kit::length_field(ui, &mut k.tolerance, &units, decimals.max(3)));
    // A tolerance typed is kept; until then it follows the cuts.
    if k.tolerance != before {
        k.from_scan = false;
        k.tolerance = k.tolerance.clamp(1e-3, 10.0);
    }
    cut(app, k);
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok((_, _, sections))) => {
            let kept: Vec<&Outline> = sections.iter().flat_map(|(_, o)| o).collect();
            let cut = sections.iter().filter(|(_, o)| !o.is_empty()).count();
            if cut == 0 {
                ui_kit::text(ui, tr("ui.scan_sections_nothing"), ui_kit::Text::Report("scan_sections_nothing", true));
            } else {
                let owned: Vec<Outline> = kept.into_iter().cloned().collect();
                let (lines, arcs, circles) = crate::dialogs_scan_section::counts(&owned);
                let note = solidcraft_ui_text::format(
                    "ui.scan_sections_found",
                    &[("sections", &cut.to_string()), ("lines", &lines.to_string()), ("arcs", &arcs.to_string()), ("circles", &circles.to_string())],
                );
                ui_kit::text(ui, &note, ui_kit::Text::Note("scan_sections_found"));
                let missed = sections.len() - cut;
                if missed > 0 {
                    let note = solidcraft_ui_text::format("ui.scan_sections_missed", &[("missed", &missed.to_string())]);
                    ui_kit::text(ui, &note, ui_kit::Text::Note("scan_sections_missed"));
                }
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(why), ui_kit::Text::Report("scan_sections_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, tr("ui.scan_sections_hint"), ui_kit::Text::Hint("scan_sections_hint"));
    false
}

/// What OK runs: the sections sketched.
pub fn commands(k: &SectionsAlong) -> Result<Vec<(String, Value)>, String> {
    let (body, _, sections) = k.shown.as_ref().ok_or("still cutting the scan")?.result.as_ref().map_err(Clone::clone)?;
    if sections.iter().all(|(_, o)| o.is_empty()) {
        return Err(tr("ui.scan_sections_nothing").into());
    }
    let mut p = json!({"body": body, "count": k.count, "every": k.every});
    if !k.from_scan {
        p["tolerance"] = json!(format!("{} mm", k.tolerance));
    }
    if let Some((sketch, id)) = &k.path {
        p["sketch"] = json!(sketch);
        p["curves"] = json!([id]);
    }
    Ok(vec![("scan.sections_along_curve".into(), p)])
}

/// Whether the dialog takes clicks on 3D curves as the curve to follow.
pub(crate) fn choosing(app: &SolveApp) -> bool {
    matches!(app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSections(_)))
}

/// A click at `at`: on a 3D sketch curve, the sections follow it.
pub(crate) fn click(app: &mut SolveApp, proj: &crate::viewport::Proj, at: egui::Pos2) {
    let hit = crate::viewport::pick_cached(app, proj, at).into_iter().find_map(|h| match h {
        crate::viewport::Hit::SketchCurve { sketch, id, .. } => Some((sketch, id)),
        _ => None,
    });
    let state = app.session.model.state();
    let wire = hit.filter(|(sketch, id)| state.sketch(*sketch).is_some_and(|ss| ss.sketch.wire_index(id).is_some()));
    drop(state);
    if let (Some(path), Some(Kind::ScanSections(k))) = (wire, app.dialog.as_mut().map(|d| &mut d.kind)) {
        k.path = Some(path);
    }
}

/// The curve and the sections over the picture.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::ScanSections(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok((_, path, sections))) = k.shown.as_ref().map(|s| &s.result) else { return };
    let t = solidcraft_theme::Tokens::get();
    let line: Vec<egui::Pos2> = path.iter().filter_map(|p| proj.to_screen(*p)).collect();
    painter.add(egui::Shape::line(line, egui::Stroke::new(1.5, t.text)));
    for (along, outlines) in sections {
        crate::dialogs_scan_section::paint(painter, proj, &along.station.plane, outlines);
        if let Some(p) = proj.to_screen(along.station.plane.origin) {
            painter.circle_filled(p, 3.0, if outlines.is_empty() { t.text_dim } else { t.accent });
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A ring (torus) about Z, its core of radius 30 and its tube of radius 5, as an STL.
    fn ring_stl() -> String {
        let (around, across) = (96u32, 32u32);
        let at = |i: u32, j: u32| {
            let (a, b) = (
                std::f64::consts::TAU * f64::from(i % around) / f64::from(around),
                std::f64::consts::TAU * f64::from(j % across) / f64::from(across),
            );
            let r = 30.0 + 5.0 * b.cos();
            [r * a.cos(), r * a.sin(), 5.0 * b.sin()]
        };
        let mut stl = String::from("solid r\n");
        for i in 0..around {
            for j in 0..across {
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

    /// With a curve drawn over the top of a ring, the dialog previews eight sections round it, each
    /// the tube's circle; four asked for, OK keeps four sketches in one undo step.
    #[test]
    fn sections_along_the_last_curve_drawn_are_previewed_and_kept() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("ring.stl");
        std::fs::write(&path, ring_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let over: Vec<[f64; 3]> = (0..6)
            .map(|k| {
                let a = std::f64::consts::TAU * f64::from(k) / 6.0;
                [30.0 * a.cos(), 30.0 * a.sin(), 5.0]
            })
            .collect();
        h.app.run("scan.curve_on_scan", json!({"points": over, "closed": true})).unwrap();
        h.app.start("scan.sections_along_curve");
        h.frames(2);
        assert!(choosing(&h.app));
        let Some(Kind::ScanSections(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the dialog") };
        let Some(Ok((_, _, sections))) = k.shown.as_ref().map(|s| &s.result) else { panic!("the preview: {:?}", k.shown) };
        assert_eq!(sections.len(), 8);
        assert!(sections.iter().all(|(_, o)| matches!(o.as_slice(), [Outline::Circle { radius, .. }] if (radius - 5.0).abs() < 0.1)), "{sections:?}");
        if let Some(Kind::ScanSections(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.count = 4;
        }
        h.frames(2);
        let before = h.app.session.doc.features.len();
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert_eq!(h.app.session.doc.features.len(), before + 4, "four sections kept");
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.doc.features.len(), before, "one undo step");
    }
}
