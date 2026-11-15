//! Align by Points (`scan.align_pairs`): click points on the scan, then their partners on what it
//! is to be laid on (another scan or a model), in the same order. From three pairs on, the dialog
//! says how far each pair is left apart by the best turn and shift, the worst one first to fix;
//! OK lays the scan so, then fits it onto the surface the partners are on.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::align::{ByPairs, pairs_preview};

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::selection::{FACES, SelInput, VERTICES, items};
use crate::ui_kit;

/// The Align by Points dialog's values (the points are its two selection inputs).
#[derive(Clone, Debug)]
pub struct AlignPairs {
    /// Fit onto the target's surface after the points.
    pub refine: bool,
}

impl Default for AlignPairs {
    fn default() -> Self {
        AlignPairs { refine: true }
    }
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.align_pairs").then(|| {
        (
            Kind::ScanAlignPairs(AlignPairs::default()),
            vec![SelInput::new("On the Scan", FACES | VERTICES, true).places(), SelInput::new("On the Target", FACES | VERTICES, true).places()],
        )
    })
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The points picked in input `k`, in order, and the one body they are all on (if one).
fn picked(inputs: &[SelInput], k: usize) -> (Vec<Vec3>, Option<String>) {
    let (mut points, mut bodies) = (Vec::new(), Vec::new());
    for x in items(inputs, k) {
        let (body, point) = match x {
            Sel::Face { body, point, .. } | Sel::Vertex { body, point } => (body, point),
            _ => continue,
        };
        points.push(*point);
        if !bodies.contains(body) {
            bodies.push(body.clone());
        }
    }
    let one = if bodies.len() == 1 { bodies.pop() } else { None };
    (points, one)
}

/// The pairs' fit, from three pairs on.
fn fit(inputs: &[SelInput]) -> Option<Result<ByPairs, String>> {
    let ((from, _), (onto, _)) = (picked(inputs, 0), picked(inputs, 1));
    (from.len().min(onto.len()) >= 3).then(|| pairs_preview(&from, &onto))
}

/// The dialog's rows (after the two inputs).
pub fn rows(_app: &SolveApp, ui: &mut egui::Ui, k: &mut AlignPairs, inputs: &[SelInput]) -> bool {
    let ((from, _), (onto, target)) = (picked(inputs, 0), picked(inputs, 1));
    if target.is_some() {
        form_row(ui, tr("ui.scan_align_refine"), |ui| check(ui, &mut k.refine));
    }
    let counts = solidcraft_ui_text::format("ui.scan_align_pairs", &[("from", &from.len().to_string()), ("onto", &onto.len().to_string())]);
    ui_kit::text(ui, &counts, ui_kit::Text::Note("scan_align_pairs"));
    match fit(inputs) {
        Some(Ok(found)) => {
            let worst = found.worst();
            for (i, r) in found.residuals.iter().enumerate() {
                let line = solidcraft_ui_text::format("ui.scan_align_residual", &[("pair", &(i + 1).to_string()), ("off", &format!("{r:.3}"))]);
                if i == worst && found.residuals.len() > 3 {
                    ui_kit::text(ui, &line, ui_kit::Text::Report("scan_align_worst", true));
                } else {
                    ui_kit::text(ui, &line, ui_kit::Text::Cell);
                }
            }
            let rms = solidcraft_ui_text::format("ui.scan_align_rms", &[("rms", &format!("{:.3}", found.rms))]);
            ui_kit::text(ui, &rms, ui_kit::Text::Note("scan_align_rms"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, &why, ui_kit::Text::Report("scan_align_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_align_hint"), ui_kit::Text::Hint("scan_align_hint"));
        }
    }
    false
}

/// What OK runs.
pub fn commands(k: &AlignPairs, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let ((from, scan), (onto, target)) = (picked(inputs, 0), picked(inputs, 1));
    let scan = scan.ok_or_else(|| tr("ui.scan_align_hint").to_owned())?;
    fit(inputs).ok_or_else(|| tr("ui.scan_align_hint").to_owned())??;
    let n = from.len().min(onto.len());
    let pts = |v: &[Vec3]| v.iter().take(n).map(|q| json!([q.x, q.y, q.z])).collect::<Vec<_>>();
    let mut p = json!({"body": scan, "from": pts(&from), "onto": pts(&onto)});
    if let Some(target) = target.filter(|t| *t != scan) {
        p["to"] = json!(target);
        p["refine"] = json!(k.refine);
    }
    Ok(vec![("scan.align_pairs".into(), p)])
}

/// Each pair numbered over the picture, the scan's point and its partner joined by a line.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(d) = app.dialog.as_ref() else { return };
    let Kind::ScanAlignPairs(_) = &d.kind else { return };
    let ((from, _), (onto, _)) = (picked(&d.inputs, 0), picked(&d.inputs, 1));
    let t = solidcraft_theme::Tokens::get();
    let font = egui::FontId::proportional(t.font_label);
    for (i, q) in from.iter().enumerate() {
        if let Some(a) = proj.to_screen(*q) {
            painter.circle_filled(a, 4.0, t.accent);
            painter.text(a + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, (i + 1).to_string(), font.clone(), t.accent);
            if let Some(b) = onto.get(i).and_then(|q| proj.to_screen(*q)) {
                painter.line_segment([a, b], egui::Stroke::new(1.0, t.accent));
            }
        }
    }
    for (i, q) in onto.iter().enumerate() {
        if let Some(b) = proj.to_screen(*q) {
            painter.circle_stroke(b, 5.0, egui::Stroke::new(2.0, t.accent));
            painter.text(b + egui::vec2(6.0, -6.0), egui::Align2::LEFT_BOTTOM, (i + 1).to_string(), font.clone(), t.accent);
        }
    }
}

/// Align (`scan.align`): the ways of aligning a scan, each a button with what it is for; a button
/// opens that tool. Close only, as Survey Scan's next steps are.
#[derive(Clone, Debug, Default)]
pub struct AlignChoose {
    /// The tool a button chose, opened on the next frame.
    open: Option<&'static str>,
}

pub fn start_choose(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.align").then(|| (Kind::ScanAlignChoose(AlignChoose::default()), Vec::new()))
}

/// The chooser's rows: a button for each way, and the help that says what it is for.
pub fn choose_rows(_app: &SolveApp, ui: &mut egui::Ui, k: &mut AlignChoose) -> bool {
    for (_, id) in solidcraft_engine::cmd::scan::align::ALIGN_METHODS {
        let label = solidcraft_ui_text::tr_label(crate::command_label(id));
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, label, ui_kit::Button::Normal)).clicked() {
            k.open = Some(id);
        }
        let help = format!("help.{id}");
        ui_kit::text(ui, solidcraft_ui_text::tr(&help), ui_kit::Text::Hint("scan_align_why"));
    }
    false
}

/// Open the tool a button chose, in the chooser's place.
pub(crate) fn tick(app: &mut SolveApp) {
    let Some(Kind::ScanAlignChoose(k)) = app.dialog.as_mut().map(|d| &mut d.kind) else { return };
    if let Some(id) = k.open.take() {
        crate::dialogs::cancel(app);
        app.start(id);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn box_stl(shift: [f64; 3]) -> String {
        let p = |i: u32| {
            [
                if i & 1 == 0 { 0.0 } else { 40.0 } + shift[0],
                if i & 2 == 0 { 0.0 } else { 30.0 } + shift[1],
                if i & 4 == 0 { 0.0 } else { 20.0 } + shift[2],
            ]
        };
        let tris =
            [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]];
        let mut out = String::from("solid b\n");
        for t in tris {
            out.push_str("facet normal 0 0 0\nouter loop\n");
            for i in t {
                let [x, y, z] = p(i);
                out.push_str(&format!("vertex {x} {y} {z}\n"));
            }
            out.push_str("endloop\nendfacet\n");
        }
        out.push_str("endsolid b\n");
        out
    }

    /// Three corners of a box moved off, picked against the same corners of the box where it
    /// belongs: the pairs fit exactly, and OK lays the moved box on the other.
    #[test]
    fn three_pairs_picked_lay_the_scan_on_the_other() {
        let mut h = crate::scenario::Harness::new();
        for (name, shift) in [("here", [0.0, 0.0, 0.0]), ("there", [70.0, 5.0, 0.0])] {
            let path = h.dir.join(format!("{name}.stl"));
            std::fs::write(&path, box_stl(shift)).unwrap();
            h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        }
        let names: Vec<String> = h.app.session.model.state().bodies.iter().map(|b| b.name.clone()).collect();
        let (here, there) = (names[0].clone(), names[1].clone());
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.align_pairs");
        let corners = [Vec3::new(0.0, 0.0, 0.0), Vec3::new(40.0, 0.0, 0.0), Vec3::new(40.0, 30.0, 20.0)];
        if let Some(d) = h.app.dialog.as_mut() {
            for q in corners {
                d.inputs[0].items.push(Sel::Vertex { body: there.clone(), point: q + Vec3::new(70.0, 5.0, 0.0) });
                d.inputs[1].items.push(Sel::Vertex { body: here.clone(), point: q });
            }
        }
        h.frames(2);
        let fitted = h.app.dialog.as_ref().and_then(|d| fit(&d.inputs)).expect("three pairs").expect("they fit");
        assert!(fitted.rms < 1e-6, "{fitted:?}");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (p, _) = state.body(&there).and_then(|b| b.body.triangle_mesh()).unwrap();
        let lo = p.iter().fold(Vec3::new(f64::MAX, f64::MAX, f64::MAX), |a, q| Vec3::new(a.x.min(q.x), a.y.min(q.y), a.z.min(q.z)));
        assert!(lo.x.abs() < 1e-6 && lo.y.abs() < 1e-6 && lo.z.abs() < 1e-6, "laid on the other: {lo:?}");
    }

    /// Align opens its chooser; a button there opens that tool in its place.
    #[test]
    fn align_chooses_a_way_and_opens_its_tool() {
        let mut h = crate::scenario::Harness::new();
        h.app.start("scan.align");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanAlignChoose(_))));
        if let Some(Kind::ScanAlignChoose(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.open = Some("scan.align_pairs");
        }
        super::tick(&mut h.app);
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanAlignPairs(_))), "Align by Points opens");
    }
}
