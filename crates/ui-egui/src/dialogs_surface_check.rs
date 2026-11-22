//! Surface Check (`inspect.surface_check`): pick bodies (or none, for every visible one); each of
//! their B-spline faces is checked as the grid changes, what is wrong listed face by face, and the
//! places marked over the view: a dot where a face folds, a ring where it pinches, a cross where it
//! passes through itself. It reads only, so OK just closes it.

use serde_json::{Value, json};
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::surface_check::checked;
use solidcraft_engine::schema::surface_check::SurfaceCheckResult;

use crate::SolveApp;
use crate::dialogs::Kind;
use crate::selection::{BODIES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The grids offered, cells each way: coarse, normal, fine.
const GRIDS: [usize; 3] = [20, 40, 80];

/// Faces listed by name before the rest are counted.
const LISTED: usize = 12;

/// A mark's size on the screen, in pixels.
const MARK: f32 = 5.0;

/// The Surface Check dialog's values (the bodies are its selection input).
#[derive(Clone, Debug)]
pub struct SurfaceCheck {
    /// Index into [`GRIDS`].
    pub grid: usize,
    shown: Option<Box<Shown>>,
}

impl Default for SurfaceCheck {
    fn default() -> Self {
        SurfaceCheck { grid: 1, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<SurfaceCheckResult, String>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "inspect.surface_check").then(|| (Kind::SurfaceCheck(SurfaceCheck::default()), vec![SelInput::new("Bodies (all if none)", BODIES, true)]))
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// What `inspect.surface_check` is asked, from the bodies picked and the grid.
fn params(k: &SurfaceCheck, inputs: &[SelInput]) -> Value {
    let mut bodies: Vec<String> = Vec::new();
    for x in items(inputs, 0) {
        if let Sel::Body { name } | Sel::Face { body: name, .. } = x
            && !bodies.contains(name)
        {
            bodies.push(name.clone());
        }
    }
    let grid = GRIDS[k.grid.min(GRIDS.len() - 1)];
    if bodies.is_empty() { json!({"grid": grid}) } else { json!({"bodies": bodies, "grid": grid}) }
}

/// Check again when the bodies, the grid or the design have changed.
fn look(app: &SolveApp, k: &mut SurfaceCheck, inputs: &[SelInput]) {
    let p = params(k, inputs);
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = checked(&app.session, &p).map_err(|e| e.to_string());
    k.shown = Some(Box::new(Shown { key, result }));
}

/// The dialog's rows (after the bodies).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut SurfaceCheck, inputs: &[SelInput]) -> bool {
    let grids = [tr("ui.surface_check_coarse"), tr("ui.surface_check_medium"), tr("ui.surface_check_fine")];
    choice_row(ui, tr("ui.surface_check_grid"), "surface_check_grid", &mut k.grid, &grids);
    look(app, k, inputs);
    let Some(shown) = k.shown.as_ref() else { return false };
    let r = match &shown.result {
        Err(why) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("surface_check_error", true));
            return false;
        }
        Ok(r) => r,
    };
    if r.checked == 0 {
        ui_kit::text(ui, tr("ui.surface_check_none"), ui_kit::Text::Note("surface_check_none"));
        return false;
    }
    let total = solidcraft_ui_text::format("ui.surface_check_result", &[("checked", &r.checked.to_string()), ("clean", &r.clean.to_string())]);
    ui_kit::text(ui, &total, ui_kit::Text::Report("surface_check_result", r.clean < r.checked));
    let bad: Vec<_> = r.faces.iter().filter(|f| !f.clean).collect();
    for f in bad.iter().take(LISTED) {
        let line = solidcraft_ui_text::format(
            "ui.surface_check_face",
            &[("body", &f.body), ("face", &f.face.to_string()), ("trouble", &trouble(f.folds, f.pinches, f.crossings))],
        );
        ui_kit::text(ui, &line, ui_kit::Text::Report("surface_check_face", true));
    }
    if bad.len() > LISTED {
        let more = solidcraft_ui_text::format("ui.surface_check_more", &[("count", &(bad.len() - LISTED).to_string())]);
        ui_kit::text(ui, &more, ui_kit::Text::Note("surface_check_more"));
    }
    // One face: how square and how even its parameters are.
    if let [f] = r.faces.as_slice() {
        let shape = solidcraft_ui_text::format(
            "ui.surface_check_shape",
            &[("angle", &format!("{:.0}", f.least_angle)), ("stretch", &format!("{:.1}", f.stretch))],
        );
        ui_kit::text(ui, &shape, ui_kit::Text::Note("surface_check_shape"));
    }
    if !bad.is_empty() {
        ui_kit::text(ui, tr("ui.surface_check_hint"), ui_kit::Text::Hint("surface_check_hint"));
    }
    false
}

/// What is wrong, only the kinds found: "Folds 3, Crossings 1".
pub(crate) fn trouble(folds: usize, pinches: usize, crossings: usize) -> String {
    [("ui.surface_check_folds", folds), ("ui.surface_check_pinches", pinches), ("ui.surface_check_crossings", crossings)]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(id, n)| solidcraft_ui_text::format(id, &[("count", &n.to_string())]))
        .collect::<Vec<_>>()
        .join(solidcraft_ui_text::tr("ui.surface_check_join"))
}

/// Under a fit's result: that the surface it made folds, pinches or passes through itself, and what
/// to do instead. Nothing when it does none of them.
pub(crate) fn fold_warning(ui: &mut egui::Ui, [folds, pinches, crossings]: [usize; 3]) {
    if folds + pinches + crossings == 0 {
        return;
    }
    let line = solidcraft_ui_text::format("ui.scan_fit_folds", &[("trouble", &trouble(folds, pinches, crossings))]);
    ui_kit::text(ui, &line, ui_kit::Text::Report("scan_fit_folds", true));
}

/// OK closes: the check changes nothing.
pub fn commands() -> Result<Vec<(String, Value)>, String> {
    Ok(Vec::new())
}

/// The places found, marked over the picture: a dot where a face folds, a ring where it pinches, a
/// cross where it passes through itself.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(Kind::SurfaceCheck(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return };
    let Some(Ok(r)) = k.shown.as_ref().map(|s| &s.result) else { return };
    for f in &r.faces {
        for place in &f.places {
            if let Some(at) = proj.to_screen(solidcore_record::Vec3::new(place.at[0], place.at[1], place.at[2])) {
                mark(painter, at, &place.kind);
            }
        }
    }
}

/// One trouble mark at `at` on the screen, by its kind (fold, pinch or crossing): its shape tells
/// it as well as its colour.
pub(crate) fn mark(painter: &egui::Painter, at: egui::Pos2, kind: &str) {
    let t = solidcraft_theme::Tokens::get();
    match kind {
        "fold" => {
            painter.circle_filled(at, MARK, t.error);
            painter.circle_stroke(at, MARK, egui::Stroke::new(1.0, t.panel));
        }
        "pinch" => {
            painter.circle_stroke(at, MARK, egui::Stroke::new(2.0, t.warning));
        }
        _ => {
            let s = egui::Stroke::new(2.0, t.error);
            painter.line_segment([at + egui::vec2(-MARK, -MARK), at + egui::vec2(MARK, MARK)], s);
            painter.line_segment([at + egui::vec2(-MARK, MARK), at + egui::vec2(MARK, -MARK)], s);
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A Free Form whose net has a column dragged back past its neighbour, so the surface folds.
    fn folded(h: &mut crate::scenario::Harness) -> String {
        let made =
            h.app.run("scan.free_form", json!({"corners": [[0, 0, 0], [50, 0, 0], [50, 50, 0], [0, 50, 0]], "fit": "off", "count": 6})).unwrap();
        let body = made["surface"].as_str().unwrap().to_owned();
        let knots = [0.0, 0.0, 0.0, 0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0, 1.0, 1.0, 1.0];
        let points: Vec<[f64; 3]> = (0..6)
            .flat_map(|j| (0..6).map(move |i| (i, j)))
            .map(|(i, j)| [if i == 3 { 0.0 } else { i as f64 * 10.0 }, j as f64 * 10.0, 0.0])
            .collect();
        h.app.run("scan.edit_surface", json!({"surface": body, "net": {"count": [6, 6], "knots": [knots, knots], "points": points}})).unwrap();
        body
    }

    fn kind(h: &crate::scenario::Harness) -> &SurfaceCheck {
        match h.app.dialog.as_ref().map(|d| &d.kind) {
            Some(Kind::SurfaceCheck(k)) => k,
            _ => panic!("the Surface Check dialog is open"),
        }
    }

    #[test]
    fn surface_check_lists_a_folded_face_and_ok_changes_nothing() {
        let mut h = crate::scenario::Harness::new();
        let body = folded(&mut h);
        let revision = h.app.session.revision;
        let features = h.app.session.doc.features.len();
        h.step(&json!({"start": "inspect.surface_check"})).unwrap();
        h.frame();
        let r = kind(&h).shown.as_ref().and_then(|s| s.result.as_ref().ok()).cloned().expect("checked");
        assert_eq!((r.checked, r.clean), (1, 0), "{r:?}");
        assert_eq!(r.faces[0].body, body);
        assert!(r.faces[0].folds > 0 && !r.faces[0].places.is_empty(), "{r:?}");

        // A finer grid checks again.
        if let Some(Kind::SurfaceCheck(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.grid = 2;
        }
        h.frame();
        let r = kind(&h).shown.as_ref().and_then(|s| s.result.as_ref().ok()).cloned().expect("checked");
        assert_eq!(r.faces[0].cells, 80 * 80);

        h.frames(2);
        h.step(&json!({"widget": "OK"})).unwrap();
        h.frames(2);
        assert!(h.app.dialog.is_none(), "OK closes it");
        assert_eq!(h.app.session.doc.features.len(), features, "nothing added");
        assert_eq!(h.app.session.revision, revision, "nothing changed");
    }
}
