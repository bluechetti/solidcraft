//! Denoise (`scan.denoise`): the study first, then the strength chosen. Opening the dialog studies
//! the scan off the window thread (every pass of the filter run and measured: the roughness left
//! and how far the surface has moved); the dialog then says how noisy the scan is, whether
//! smoothing helps and how hard it suggests, and for the strength chosen how rough the scan would
//! be and how far it would move. The roughness and movement over the passes are drawn, the
//! suggested and chosen strengths marked. OK smooths the scan at that strength, one undo step.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use solidcraft_engine::cmd::scan::mesh::{self as engine_mesh, MOST_PASSES, Study};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit;

/// The plot's height, in points.
const PLOT_HEIGHT: f32 = 72.0;

/// What the study has come to: the scan studied and the design's revision, and the study (or why
/// none).
type Outcome = (String, u64, Result<Study, String>);

/// The Denoise dialog's values.
#[derive(Clone, Debug)]
pub struct Denoise {
    /// How hard, in passes.
    pub strength: f64,
    /// The strength has been set by hand: it no longer follows the suggestion.
    chosen: bool,
    /// The study, filled in by its thread.
    study: Arc<Mutex<Option<Outcome>>>,
    /// What is being studied: the design's revision, the area asked for and the selection's stamp.
    asked: Option<String>,
    /// Which part of the scan: the whole scan unless another way is chosen.
    pub area: crate::scan_area::Area,
    /// The area of the step being edited, as it keeps it: kept as it is.
    pub kept: Option<Value>,
    /// The area chosen, marked on the scan.
    pub marks: crate::dialogs_scan_clean::AreaMarks,
}

impl Default for Denoise {
    fn default() -> Self {
        Denoise {
            strength: 0.0,
            chosen: false,
            study: Arc::new(Mutex::new(None)),
            asked: None,
            area: crate::scan_area::Area::clean(),
            kept: None,
            marks: crate::dialogs_scan_clean::AreaMarks::default(),
        }
    }
}

impl Denoise {
    /// A strength set already (a Denoise step edited): it does not follow the suggestion.
    pub fn chosen(strength: f64) -> Self {
        Denoise { strength: strength.clamp(0.0, MOST_PASSES as f64), chosen: true, ..Denoise::default() }
    }
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.denoise")
        .then(|| (Kind::ScanDenoise(Denoise::default()), vec![crate::selection::SelInput::new("Point", crate::selection::FACES, false)]))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The study, once in: the scan, and the study or why none.
fn outcome(k: &Denoise) -> Option<(String, Result<Study, String>)> {
    let slot = k.study.lock().ok()?;
    slot.as_ref().map(|(body, _, study)| (body.clone(), study.clone()))
}

/// Study the scan (its area, when one is chosen) again when the design or the area has changed: a
/// copy of its mesh studied beside the window, the area found there too (in the browser, here).
/// `Err` with what the area waits for.
fn study(app: &SolveApp, k: &mut Denoise, inputs: &[crate::selection::SelInput]) -> Result<(), String> {
    let revision = app.session.revision;
    let area = crate::dialogs_scan_clean::area_params(app, &k.area, k.kept.as_ref(), inputs)?;
    let key = format!("{revision}|{area}|{}", solidcraft_engine::cmd::scan::select::stamp(&app.session));
    if k.asked.as_ref() == Some(&key) {
        return Ok(());
    }
    k.asked = Some(key);
    let slot = Arc::new(Mutex::new(None));
    k.study = slot.clone();
    let (body, mesh) = match engine_mesh::denoise_mesh(&app.session, area.get("body").and_then(Value::as_str)) {
        Ok(mesh) => mesh,
        Err(e) => {
            if let Ok(mut s) = slot.lock() {
                *s = Some((String::new(), revision, Err(e.to_string())));
            }
            return Ok(());
        }
    };
    // The area is found on a copy of the design, beside the window: growing it over a large scan
    // takes a while.
    let copy = (area.as_object().is_some_and(|o| !o.is_empty())).then(|| app.session.scratch());
    let work = move || {
        let flags = match &copy {
            Some(copy) => match solidcraft_engine::cmd::scan::flatten::step_area(copy, &area, &body, "scan.denoise") {
                Ok(found) => found.map(|a| a.flags()),
                Err(e) => {
                    if let Ok(mut s) = slot.lock() {
                        *s = Some((body, revision, Err(e.to_string())));
                    }
                    return;
                }
            },
            None => None,
        };
        let found = engine_mesh::denoise_study_within(&mesh.positions, &mesh.triangles, flags.as_deref())
            .ok_or_else(|| tr("ui.scan_denoise_empty").to_owned());
        if let Ok(mut s) = slot.lock() {
            *s = Some((body, revision, found));
        }
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = std::thread::Builder::new().name("denoise study".into()).spawn(work);
    }
    #[cfg(target_arch = "wasm32")]
    work();
    Ok(())
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Denoise, inputs: &[crate::selection::SelInput]) -> bool {
    crate::dialogs_scan_clean::area_row(app, ui, "scan_denoise_area", &mut k.area, k.kept.as_ref());
    crate::dialogs_scan_clean::mark_area(app, &k.area, k.kept.as_ref(), inputs, &mut k.marks);
    if let Err(waiting) = study(app, k, inputs) {
        ui_kit::text(ui, &waiting, ui_kit::Text::Hint("scan_denoise_area_hint"));
        return false;
    }
    let Some((body, found)) = outcome(k) else {
        ui_kit::text(ui, tr("ui.scan_denoise_studying"), ui_kit::Text::Note("scan_denoise_studying"));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return false;
    };
    let found = match found {
        Ok(found) => found,
        Err(why) => {
            ui_kit::text(ui, &crate::dialogs::shown_error(&why), ui_kit::Text::Report("scan_denoise_error", true));
            return false;
        }
    };
    if !k.chosen {
        k.strength = found.suggested;
    }
    // Scan noise is hundredths of a millimetre: shown to four places at least.
    let units = &app.session.doc.units;
    let mm = |v: f64| crate::prefs::format_length(v, units, app.preferences.length_decimals.max(4));
    form_row(ui, tr("ui.scan_denoise_scan"), |ui| ui_kit::text(ui, &body, ui_kit::Text::Cell));
    form_row(ui, tr("ui.scan_denoise_noise"), |ui| ui_kit::text(ui, &mm(found.noise), ui_kit::Text::Cell));
    let verdict = if found.suggested > 0.0 {
        solidcraft_ui_text::format("ui.scan_denoise_suggests", &[("strength", &format!("{:.2}", found.suggested))])
    } else {
        tr("ui.scan_denoise_smooth_already").to_owned()
    };
    ui_kit::text(ui, &verdict, ui_kit::Text::Note("scan_denoise_verdict"));
    let before = k.strength;
    form_row(ui, tr("ui.scan_denoise_strength"), |ui| {
        ui_kit::slider(ui, &mut k.strength, 0.0..=MOST_PASSES as f64, ui_kit::Slider { step: Some(0.05), ..ui_kit::Slider::default() })
    });
    if found.suggested > 0.0
        && ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_denoise_use_suggested"), ui_kit::Button::Enabled(k.strength != found.suggested)))
            .clicked()
    {
        k.strength = found.suggested;
    }
    k.strength = k.strength.clamp(0.0, MOST_PASSES as f64);
    if k.strength != before {
        k.chosen = true;
    }
    let at = found.at(k.strength);
    let share = if found.noise > 0.0 { 100.0 * (1.0 - at.noise / found.noise) } else { 0.0 };
    let rough = solidcraft_ui_text::format("ui.scan_denoise_roughness_value", &[("roughness", &mm(at.noise)), ("less", &format!("{share:.0}"))]);
    form_row(ui, tr("ui.scan_denoise_roughness"), |ui| ui_kit::text(ui, &rough, ui_kit::Text::Cell));
    let moved = solidcraft_ui_text::format("ui.scan_denoise_moved_value", &[("rms", &mm(at.rms_moved))]);
    form_row(ui, tr("ui.scan_denoise_moved"), |ui| ui_kit::text(ui, &moved, ui_kit::Text::Cell));
    form_row(ui, tr("ui.scan_denoise_most"), |ui| ui_kit::text(ui, &mm(at.max_moved), ui_kit::Text::Cell));
    plot(ui, &found, k.strength);
    ui_kit::text(ui, tr("ui.scan_denoise_hint"), ui_kit::Text::Hint("scan_denoise_hint"));
    false
}

/// The roughness (falling) and the movement (rising) over the passes, each on its own scale, the
/// suggested strength a dot on the roughness and the chosen one a line across.
fn plot(ui: &mut egui::Ui, found: &Study, strength: f64) {
    let t = solidcraft_theme::Tokens::get();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), PLOT_HEIGHT), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_stroke(rect, 2.0, egui::Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
    let inner = rect.shrink(6.0);
    let x = |s: f64| inner.left() + inner.width() * (s / MOST_PASSES as f64) as f32;
    let top = |v: f64, most: f64| inner.bottom() - inner.height() * if most > 0.0 { (v / most) as f32 } else { 0.0 };
    let most_rough = found.stages.iter().map(|s| s.roughness).fold(0.0, f64::max);
    let most_moved = found.stages.iter().map(|s| s.rms_moved).fold(0.0, f64::max);
    let line = |values: Vec<f64>, most: f64, colour: egui::Color32| {
        let points: Vec<egui::Pos2> = values.iter().enumerate().map(|(k, v)| egui::pos2(x(k as f64), top(*v, most))).collect();
        painter.add(egui::Shape::line(points, egui::Stroke::new(2.0, colour)));
    };
    line(found.stages.iter().map(|s| s.roughness).collect(), most_rough, t.accent);
    line(found.stages.iter().map(|s| s.rms_moved).collect(), most_moved, t.sketch_accent);
    painter.line_segment([egui::pos2(x(strength), inner.top()), egui::pos2(x(strength), inner.bottom())], egui::Stroke::new(1.0, t.text));
    if found.suggested > 0.0 {
        painter.circle_filled(egui::pos2(x(found.suggested), top(found.at(found.suggested).roughness, most_rough)), 3.5, t.accent);
    }
    ui.horizontal(|ui| {
        for (label, colour) in [(tr("ui.scan_denoise_plot_roughness"), t.accent), (tr("ui.scan_denoise_plot_moved"), t.sketch_accent)] {
            let (key, _) = ui.allocate_exact_size(egui::vec2(14.0, 10.0), egui::Sense::hover());
            ui.painter().line_segment([key.left_center(), key.right_center()], egui::Stroke::new(2.0, colour));
            ui_kit::text(ui, label, ui_kit::Text::Inline);
        }
    });
}

/// What OK runs: the scan smoothed at the strength chosen.
pub fn commands(app: &SolveApp, k: &Denoise, inputs: &[crate::selection::SelInput]) -> Result<Vec<(String, Value)>, String> {
    let mut p = crate::dialogs_scan_clean::area_params(app, &k.area, k.kept.as_ref(), inputs)?;
    let (body, found) = outcome(k).ok_or_else(|| tr("ui.scan_denoise_studying").to_owned())?;
    found?;
    if !(k.strength > 0.0) {
        return Err(tr("ui.scan_denoise_choose").into());
    }
    p["body"] = json!(body);
    p["strength"] = json!((k.strength * 100.0).round() / 100.0);
    Ok(vec![("scan.denoise".into(), p)])
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A plate 40 × 30 × 6, its faces cut fine and shaken along their normals, as a binary STL.
    fn noisy_plate_stl(path: &std::path::Path) {
        let (n, size) = (40u32, [40.0f64, 30.0, 6.0]);
        let shake = |i: u64| {
            let s = ((i as f64) * 12.9898).sin() * 43_758.545;
            0.06 * (s - s.floor() - 0.5)
        };
        let mut triangles: Vec<[[f64; 3]; 3]> = Vec::new();
        // Each face a grid: `a` and `b` its two directions, `c` the axis across it at `at`.
        for (a, b, c, at, flip) in [
            (0, 1, 2, 0.0, true),
            (0, 1, 2, size[2], false),
            (0, 2, 1, 0.0, false),
            (0, 2, 1, size[1], true),
            (1, 2, 0, 0.0, true),
            (1, 2, 0, size[0], false),
        ] {
            let point = |i: u32, j: u32| {
                let mut p = [0.0; 3];
                p[a] = size[a] * f64::from(i) / f64::from(n);
                p[b] = size[b] * f64::from(j) / f64::from(n);
                p[c] = at;
                let edge = i == 0 || j == 0 || i == n || j == n;
                if !edge {
                    p[c] += shake(u64::from(i) * 7919 + u64::from(j) * 104_729 + c as u64 * 31 + at as u64);
                }
                p
            };
            for i in 0..n {
                for j in 0..n {
                    let (p, q, r, s) = (point(i, j), point(i + 1, j), point(i + 1, j + 1), point(i, j + 1));
                    if flip {
                        triangles.push([p, r, q]);
                        triangles.push([p, s, r]);
                    } else {
                        triangles.push([p, q, r]);
                        triangles.push([p, r, s]);
                    }
                }
            }
        }
        let mut bytes = vec![0u8; 80];
        bytes.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
        for t in &triangles {
            bytes.extend_from_slice(&[0u8; 12]);
            for p in t {
                for v in p {
                    bytes.extend_from_slice(&(*v as f32).to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0u8; 2]);
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn dialog(h: &crate::scenario::Harness) -> &Denoise {
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::ScanDenoise(k)) => k,
            _ => panic!("the Denoise dialog"),
        }
    }

    /// The dialog studies a noisy plate beside the window and suggests a strength; a strength
    /// chosen by hand reads its roughness and movement from the study, and OK smooths the scan at
    /// it, one undo step.
    #[test]
    fn the_study_suggests_a_strength_and_ok_smooths_at_the_one_chosen() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("plate.stl");
        noisy_plate_stl(&path);
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.denoise");
        for _ in 0..600 {
            h.frames(1);
            if outcome(dialog(&h)).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        h.frames(2);
        let (body, found) = outcome(dialog(&h)).expect("the study is in");
        let found = found.expect("a study");
        assert!(found.suggested > 0.0, "a noisy plate is worth smoothing: {found:?}");
        assert_eq!(dialog(&h).strength, found.suggested, "the strength starts at the suggestion");
        if let Some(Kind::ScanDenoise(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.strength = 1.0;
            k.chosen = true;
        }
        h.frames(2);
        assert_eq!(dialog(&h).strength, 1.0, "a strength chosen stays");
        let inputs = h.app.dialog.as_ref().map(|d| d.inputs.clone()).unwrap_or_default();
        let commands = commands(&h.app, dialog(&h), &inputs).unwrap();
        assert_eq!(commands, vec![("scan.denoise".to_owned(), json!({"body": body, "strength": 1.0}))], "the whole scan, as before");
        assert!(crate::dialogs_scan_clean::gpu_area_marks(&h.app).is_none(), "the whole scan is not marked");
        let before = h.app.session.revision;
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.revision > before, "the scan is smoothed");
        h.app.run("edit.undo", json!({})).unwrap();
    }

    /// Round the click: the study and OK work on the scan near the click alone, and nothing else
    /// of the scan moves.
    #[test]
    fn round_the_click_smooths_only_there() {
        use solidcraft_engine::geom::prelude::*;
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("plate.stl");
        noisy_plate_stl(&path);
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        let positions = |h: &crate::scenario::Harness| {
            h.app.session.model.state().body(&scan).and_then(|b| b.body.triangle_mesh()).map(|(p, _)| p.to_vec()).unwrap_or_default()
        };
        let before = positions(&h);
        let (lo, hi) = before.iter().fold((before[0], before[0]), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        let centre = (lo + hi) * 0.5;
        h.app.start("scan.denoise");
        if let Some(d) = h.app.dialog.as_mut() {
            d.inputs[0].items.push(solidcraft_engine::Sel::Face { body: scan.clone(), index: 0, point: centre });
            if let Kind::ScanDenoise(k) = &mut d.kind {
                k.area.way = crate::scan_area::ROUND;
                k.area.radius = 0.2 * (hi - lo).len();
                k.strength = 2.0;
                k.chosen = true;
            }
        }
        for _ in 0..2000 {
            h.frames(1);
            if outcome(dialog(&h)).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        outcome(dialog(&h)).expect("the area studied").1.expect("a study");
        // The area is marked on the scan, found beside the window too.
        for _ in 0..2000 {
            if crate::dialogs_scan_clean::gpu_area_marks(&h.app).is_some() {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let marks = crate::dialogs_scan_clean::gpu_area_marks(&h.app).expect("the area marked");
        assert!(!marks.dots.is_empty());
        let inputs = h.app.dialog.as_ref().map(|d| d.inputs.clone()).unwrap_or_default();
        let sent = commands(&h.app, dialog(&h), &inputs).unwrap();
        assert!(sent[0].1.get("radius").is_some() && sent[0].1.get("point").is_some(), "{sent:?}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        // OK smooths beside the window: on a busy machine it takes a while to come in.
        for _ in 0..2000 {
            if h.app.dialog.is_none() {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let after = positions(&h);
        let reach = 0.2 * (hi - lo).len() + 0.1 * (hi - lo).len();
        assert!(before.iter().zip(&after).filter(|(a, _)| a.dist(centre) > reach).all(|(a, b)| a == b), "far from the click nothing moved");
        assert!(before.iter().zip(&after).any(|(a, b)| a != b), "near it the scan is smoothed");
    }
}
