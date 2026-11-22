//! Edit Surface's shape tools on the points chosen: hold them to a plane, a line, a circle, a
//! sphere, a cylinder or a cone; the edge tools (choose an edge, the row between two points, equal
//! point counts so two corners can be welded); Suggest Refinement (where the scan is far from a
//! face, the fixes and the distance each would leave); and Linear and Circular patterns.

use solidcraft_engine::cmd::scan::surface::{constrain, face_options, pattern};

use super::*;

/// What the shape tools remember: what Constrain last held the points to, the refinements Suggest
/// Refinement offers, and the pattern's axis (x, y, z), copies and step (mm for Linear, degrees
/// for Circular).
#[derive(Clone, Debug)]
pub(super) struct State {
    pub note: Option<String>,
    pub suggestions: Vec<face_options::Suggestion>,
    pub axis: usize,
    pub count: f32,
    pub step: f64,
}

impl Default for State {
    fn default() -> Self {
        State { note: None, suggestions: Vec::new(), axis: 2, count: 3.0, step: 20.0 }
    }
}

/// The shapes the chosen points can be held to, with the buttons' ids.
const SHAPES: [(constrain::Kind, &str); 6] = [
    (constrain::Kind::Plane, "ui.scan_edit_constrain_plane"),
    (constrain::Kind::Line, "ui.scan_edit_constrain_line"),
    (constrain::Kind::Circle, "ui.scan_edit_constrain_circle"),
    (constrain::Kind::Sphere, "ui.scan_edit_constrain_sphere"),
    (constrain::Kind::Cylinder, "ui.scan_edit_constrain_cylinder"),
    (constrain::Kind::Cone, "ui.scan_edit_constrain_cone"),
];

/// The points chosen, put on `kind` fitted to them (or on the simplest shape within `tolerance`
/// for `None`): one step of Undo Last Change. Says what was fitted.
pub(super) fn constrain_to(k: &mut EditSurface, kind: Option<constrain::Kind>, tolerance: f64) -> Result<(), String> {
    let ids = select::chosen_ids(k);
    let places: Vec<Vec3> = ids.iter().filter_map(|&i| select::handle_place(k, i)).collect();
    let nothing = || tr("ui.scan_edit_nothing").to_owned();
    let (kind, fit) = match kind {
        Some(kind) => {
            if places.len() < kind.min_points() {
                return Err(solidcraft_ui_text::format("ui.scan_edit_constrain_few", &[("count", &kind.min_points().to_string())]));
            }
            (kind, constrain::fit(kind, &places).ok_or_else(nothing)?)
        }
        None => constrain::best(&places, tolerance).ok_or_else(nothing)?,
    };
    let to = constrain::project_all(&fit.primitive, &places);
    let moves: Vec<(usize, Vec3)> = ids.into_iter().zip(to).collect();
    // Exactly where the shape puts them: no soft selection.
    let falloff = std::mem::replace(&mut k.falloff, 0.0);
    move_many(k, &moves);
    k.falloff = falloff;
    let name = SHAPES.iter().find(|s| s.0 == kind).map_or("", |s| tr(s.1));
    k.shape.note = Some(solidcraft_ui_text::format("ui.scan_edit_constrained_value", &[("shape", name), ("rms", &format!("{:.3}", fit.rms))]));
    Ok(())
}

/// The open sides (a face and its side) of the network the point `p` is on.
fn open_sides_of(k: &EditSurface, p: usize) -> Vec<(usize, Side)> {
    k.joins.open.iter().copied().filter(|&(f, side)| k.joins.side(&k.faces, f, side).contains(&p)).collect()
}

/// Point counts made equal between the open sides the two points chosen are on.
fn match_counts(k: &mut EditSurface) -> Result<(), String> {
    let ids = select::chosen_ids(k);
    let [a, b] = ids.as_slice() else { return Err(tr("ui.scan_edit_nothing").to_owned()) };
    let (sa, sb) = (open_sides_of(k, *a), open_sides_of(k, *b));
    let pair = sa.iter().flat_map(|x| sb.iter().map(move |y| (*x, *y))).find(|(x, y)| x != y);
    let Some((x, y)) = pair else { return Err(tr("ui.scan_edit_match_open").to_owned()) };
    let faces = pattern::match_counts(&k.faces, x, y).map_err(|e| e.to_string())?;
    reshaped_to(k, faces);
    Ok(())
}

/// As [`repeated`], for the tests.
#[cfg(test)]
pub(super) fn repeated_for_test(k: &mut EditSurface, circular: bool) -> Result<(), String> {
    repeated(k, circular)
}

/// The Free Form's middle and the faces repeated: `count` copies.
fn repeated(k: &mut EditSurface, circular: bool) -> Result<(), String> {
    let first = k.net.first().copied().ok_or_else(|| tr("ui.scan_edit_nothing").to_owned())?;
    let (lo, hi) = k.net.iter().fold((first, first), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let middle = (lo + hi) * 0.5;
    let axis = [Vec3::X, Vec3::Y, Vec3::Z].get(k.shape.axis).copied().unwrap_or(Vec3::Z);
    let count = k.shape.count.round().clamp(2.0, pattern::MOST_COPIES as f32) as usize;
    let made = if circular {
        pattern::circular(&k.faces, &pattern::Circular { axis_point: middle, axis, angle: k.shape.step, count })
    } else {
        pattern::linear(&k.faces, &pattern::Linear { direction: axis, spacing: k.shape.step, count })
    };
    let faces = made.map_err(|e| e.to_string())?;
    reshaped_to(k, faces);
    k.restudy = true;
    Ok(())
}

/// The fixes that lower the scan's distance from the faces, for the faces chosen (else the worst).
fn suggest(k: &mut EditSurface) -> Result<(), String> {
    let points = loaded(k).and_then(|l| l.covered.as_ref().map(|c| c.points.clone())).ok_or_else(|| tr("ui.scan_edit_no_scan").to_owned())?;
    let reach = k.faces.iter().map(edit::default_reach).fold(1.0, f64::max);
    let mut found = face_options::suggest(&k.faces, &points, reach, k.faces.len().min(8));
    let touched = select::faces_touched(k);
    if !k.picked.is_empty() {
        found.retain(|s| touched.contains(&s.face));
    }
    found.sort_by(|a, b| (a.rms / a.rms_before.max(1e-12)).total_cmp(&(b.rms / b.rms_before.max(1e-12))));
    found.truncate(6);
    if found.is_empty() {
        k.shape.suggestions.clear();
        return Err(tr("ui.scan_edit_suggest_none").to_owned());
    }
    k.shape.suggestions = found;
    Ok(())
}

fn fix_label(fix: face_options::Fix) -> &'static str {
    match fix {
        face_options::Fix::RefineU => tr("ui.scan_edit_fix_refine_u"),
        face_options::Fix::RefineV => tr("ui.scan_edit_fix_refine_v"),
        face_options::Fix::RefineBoth => tr("ui.scan_edit_fix_refine_both"),
        face_options::Fix::SplitU => tr("ui.scan_edit_fix_split_u"),
        face_options::Fix::SplitV => tr("ui.scan_edit_fix_split_v"),
    }
}

/// The tools' rows, under the choice's own.
pub(super) fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut EditSurface, scan: bool) {
    let mut outcome: Option<Result<(), String>> = None;
    let count = k.picked.len();
    // Hold the chosen points to a shape.
    ui_kit::text(ui, tr("ui.scan_edit_constrain"), ui_kit::Text::Hint("scan_edit_constrain"));
    for pair in SHAPES.chunks(2) {
        ui_kit::row(ui, "", |ui| {
            ui.horizontal(|ui| {
                for (kind, id) in pair {
                    let can = count >= kind.min_points();
                    let style = if can { ui_kit::Button::Small } else { ui_kit::Button::Enabled(false) };
                    if ui_kit::button(ui, tr(id), style).clicked() && can {
                        outcome = Some(constrain_to(k, Some(*kind), 0.0));
                    }
                }
            })
            .response
        });
    }
    ui_kit::row(ui, "", |ui| {
        ui.horizontal(|ui| {
            let can = count >= 3;
            let style = if can { ui_kit::Button::Small } else { ui_kit::Button::Enabled(false) };
            if ui_kit::button(ui, tr("ui.scan_edit_constrain_best"), style).clicked() && can {
                outcome = Some(constrain_to(k, None, k.tolerance));
            }
        })
        .response
    });
    if let Some(note) = &k.shape.note {
        ui_kit::text(ui, note, ui_kit::Text::Note("scan_edit_shape_note"));
    }
    // The edge tools.
    ui_kit::row(ui, "", |ui| {
        ui.horizontal(|ui| {
            let on_edge = k.chosen.is_some_and(|p| !open_sides_of(k, p).is_empty());
            let style = if on_edge { ui_kit::Button::Small } else { ui_kit::Button::Enabled(false) };
            if ui_kit::button(ui, tr("ui.scan_edit_edge_chain"), style).clicked()
                && let Some(p) = k.chosen.filter(|_| on_edge)
            {
                let chain = pattern::edge_chain(&k.joins, &k.faces, p);
                select::pick_many(k, &chain, select::How::Add);
            }
            let two = count == 2;
            let style = if two { ui_kit::Button::Small } else { ui_kit::Button::Enabled(false) };
            if ui_kit::button(ui, tr("ui.scan_edit_row_between"), style).clicked()
                && let [a, b] = select::chosen_ids(k).as_slice()
                && let Some(line) = pattern::loop_through(&k.joins, &k.faces, *a, *b)
            {
                select::pick_many(k, &line, select::How::Add);
            }
        })
        .response
    });
    ui_kit::row(ui, "", |ui| {
        let style = if count == 2 { ui_kit::Button::Normal } else { ui_kit::Button::Enabled(false) };
        ui_kit::button(ui, tr("ui.scan_edit_match_counts"), style)
    })
    .clicked()
    .then(|| outcome = Some(match_counts(k)));
    // Where the scan is far from a face: the fixes and what each would leave.
    if scan {
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_edit_suggest"), ui_kit::Button::Normal)).clicked() {
            outcome = Some(suggest(k));
        }
        let mut chosen: Option<usize> = None;
        for (n, s) in k.shape.suggestions.iter().enumerate() {
            let line = solidcraft_ui_text::format(
                "ui.scan_edit_suggestion_value",
                &[
                    ("face", &(s.face + 1).to_string()),
                    ("fix", fix_label(s.fix)),
                    ("before", &crate::prefs::show_mm(app, s.rms_before, 2)),
                    ("after", &crate::prefs::show_mm(app, s.rms, 2)),
                ],
            );
            if ui_kit::row(ui, "", |ui| ui_kit::button(ui, &line, ui_kit::Button::Small)).clicked() {
                chosen = Some(n);
            }
        }
        if let Some(n) = chosen {
            let faces = k.shape.suggestions.get(n).map(|s| s.faces.clone());
            if let Some(faces) = faces {
                reshaped_to(k, faces);
                k.shape.suggestions.clear();
                outcome = Some(Ok(()));
            }
        }
    }
    // Patterns: copies of the Free Form about its middle, in a line or round an axis.
    let axes = [tr("ui.scan_edit_axis_x"), tr("ui.scan_edit_axis_y"), tr("ui.scan_edit_axis_z")];
    choice_row(ui, tr("ui.scan_edit_pattern_axis"), "scan_edit_pattern_axis", &mut k.shape.axis, &axes);
    form_row(ui, tr("ui.scan_edit_pattern_count"), |ui| ui_kit::number_field(ui, &mut k.shape.count, 2.0..=64.0, 0.2, ""));
    form_row(ui, tr("ui.scan_edit_pattern_step"), |ui| ui_kit::number_field(ui, &mut k.shape.step, -100_000.0..=100_000.0, 0.5, ""));
    if !k.shape.step.is_finite() {
        k.shape.step = 0.0;
    }
    ui_kit::row(ui, "", |ui| {
        ui.horizontal(|ui| {
            if ui_kit::button(ui, tr("ui.scan_edit_pattern_linear"), ui_kit::Button::Small).clicked() {
                outcome = Some(repeated(k, false));
            }
            if ui_kit::button(ui, tr("ui.scan_edit_pattern_circular"), ui_kit::Button::Small).clicked() {
                outcome = Some(repeated(k, true));
            }
        })
        .response
    });
    match outcome {
        Some(Err(why)) => k.note = Some(why),
        Some(Ok(())) => k.note = None,
        None => {}
    }
}
