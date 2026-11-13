//! Survey Scan (`scan.survey`): a first look at a scan and what to do next. The button reads the
//! scan (beside the window on a big one) and the dialog shows what it found: the scan's size along
//! its own directions, its noise, holes, floating pieces and edges to repair, and, once Find All
//! has read it, how much of it is planes, cylinders and other shapes. Below that are the next
//! steps in the order the work goes, each with its reason and a button that opens that tool. It
//! changes nothing; Close closes it.

use serde_json::{Value, json};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit;

/// The Survey dialog's values: the tool asked for from a next step, and whether the scan should be
/// read again (it changed since).
#[derive(Clone, Debug, Default)]
pub struct Survey {
    open: Option<String>,
    again: bool,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.survey").then(|| (Kind::ScanSurvey(Survey::default()), Vec::new()))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// Read the scan as the dialog opens: false (and nothing opens) when it cannot be read, such as
/// with no scan in the design.
pub fn read_first(app: &mut SolveApp) -> bool {
    match app.run_heavy("scan.survey", json!({})) {
        Ok(_) => true,
        Err(why) => {
            app.set_status(why, true);
            false
        }
    }
}

/// The survey of the scan as it is now, once read.
fn kept(app: &SolveApp) -> Option<Value> {
    app.session.scan_survey.as_ref().filter(|k| k.current(&app.session)).map(|k| k.reply.clone())
}

pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Survey) -> bool {
    if app.computing_command() == Some("scan.survey") {
        ui_kit::text(ui, tr("ui.scan_survey_reading"), ui_kit::Text::Note("scan_survey_reading"));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return false;
    }
    let Some(r) = kept(app) else {
        ui_kit::text(ui, tr("ui.scan_survey_changed"), ui_kit::Text::Note("scan_survey_changed"));
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, tr("ui.scan_survey_read_again"), ui_kit::Button::Normal)).clicked() {
            k.again = true;
        }
        return false;
    };
    let units = &app.session.doc.units;
    let fine = |v: f64| crate::prefs::format_length(v, units, app.preferences.length_decimals.max(4));
    let count = |key: &str| r[key].as_u64().unwrap_or(0);
    form_row(ui, tr("ui.scan_survey_scan"), |ui| ui_kit::text(ui, r["body"].as_str().unwrap_or_default(), ui_kit::Text::Cell));
    // Each side bare, the unit once: three lengths fit the cell.
    let side = |i: usize| r["own_size"][i].as_f64().unwrap_or(0.0);
    let short = |v: f64| crate::prefs::length_number(v, units, 1);
    let size = format!("{} × {} × {}", short(side(0)), short(side(1)), crate::prefs::format_length(side(2), units, 1));
    form_row(ui, tr("ui.scan_survey_size"), |ui| ui_kit::text(ui, &size, ui_kit::Text::Cell));
    form_row(ui, tr("ui.scan_survey_triangles"), |ui| ui_kit::text(ui, &count("triangles").to_string(), ui_kit::Text::Cell));
    if let Some(noise) = r["noise"].as_f64() {
        form_row(ui, tr("ui.scan_survey_noise"), |ui| ui_kit::text(ui, &fine(noise), ui_kit::Text::Cell));
    }
    form_row(ui, tr("ui.scan_survey_holes"), |ui| ui_kit::text(ui, &count("holes").to_string(), ui_kit::Text::Cell));
    form_row(ui, tr("ui.scan_survey_floating"), |ui| ui_kit::text(ui, &count("floating").to_string(), ui_kit::Text::Cell));
    if count("tangled_edges") > 0 {
        form_row(ui, tr("ui.scan_survey_tangled"), |ui| ui_kit::text(ui, &count("tangled_edges").to_string(), ui_kit::Text::Cell));
    }
    let read = match r.get("shapes") {
        Some(sh) => crate::i18n::format("ui.scan_survey_in_shapes", &[("share", &format!("{:.0}", 100.0 * sh["in_shapes"].as_f64().unwrap_or(0.0)))]),
        None if r["sheet_like"] == true => tr("ui.scan_survey_sheet").to_owned(),
        None => tr("ui.scan_survey_not_read").to_owned(),
    };
    form_row(ui, tr("ui.scan_survey_kind"), |ui| ui_kit::text(ui, &read, ui_kit::Text::Cell));
    if let Some(hint) = r["units_hint"].as_str() {
        ui_kit::text(ui, hint, ui_kit::Text::Report("scan_survey_units", true));
    }
    ui_kit::text(ui, tr("ui.scan_survey_next"), ui_kit::Text::Heading("scan_survey_next"));
    let steps = r["next"].as_array().cloned().unwrap_or_default();
    if steps.is_empty() {
        ui_kit::text(ui, tr("ui.scan_survey_nothing_next"), ui_kit::Text::Note("scan_survey_nothing_next"));
    }
    for step in &steps {
        let Some(id) = step["command"].as_str() else { continue };
        let label = crate::i18n::tr_label(crate::command_label(id));
        if ui_kit::row(ui, "", |ui| ui_kit::button(ui, label, ui_kit::Button::Normal)).clicked() {
            k.open = Some(id.to_owned());
        }
        if let Some(why) = step["why"].as_str() {
            ui_kit::text(ui, &named(why), ui_kit::Text::Hint("scan_survey_why"));
        }
    }
    false
}

/// `why` with each command id in it (`scan.reduce`) said as the tool's name people see.
fn named(why: &str) -> String {
    let mut out = String::with_capacity(why.len());
    let mut rest = why;
    while let Some(at) = rest.find("scan.").or_else(|| rest.find("mesh.")) {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let end = from.find(|c: char| !(c.is_ascii_lowercase() || c == '_' || c == '.')).unwrap_or(from.len());
        let id = from[..end].trim_end_matches('.');
        if solidcraft_engine::find_command(id).is_some() {
            out.push_str(crate::i18n::tr_label(crate::command_label(id)));
        } else {
            out.push_str(id);
        }
        rest = &from[id.len()..];
    }
    out.push_str(rest);
    out
}

/// Each frame: open the tool a next step asked for (the survey closes), or read the scan again.
pub(crate) fn tick(app: &mut SolveApp) {
    let Some(d) = app.dialog.as_mut() else { return };
    let Kind::ScanSurvey(k) = &mut d.kind else { return };
    let (open, again) = (k.open.take(), std::mem::take(&mut k.again));
    if let Some(id) = open {
        crate::dialogs::cancel(app);
        app.start(&id);
    } else if again {
        read_first(app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box 40 × 20 × 10 mm away from the origin, inserted as a scan.
    fn scan(h: &mut crate::scenario::Harness) {
        let (n, size, at) = (8u32, [40.0, 20.0, 10.0], [100.0, 0.0, 0.0]);
        let mut stl = String::from("solid s\n");
        for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for j in 0..n {
                for i in 0..n {
                    let corner = |di: u32, dj: u32| {
                        let mut p = at;
                        if high {
                            p[axis] += size[axis];
                        }
                        p[u] += size[u] * f64::from(i + di) / f64::from(n);
                        p[v] += size[v] * f64::from(j + dj) / f64::from(n);
                        p
                    };
                    let quad = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
                    for tri in [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]] {
                        let tri = if high { tri } else { [tri[0], tri[2], tri[1]] };
                        stl += "facet normal 0 0 0\nouter loop\n";
                        for [x, y, z] in tri {
                            stl += &format!("vertex {x} {y} {z}\n");
                        }
                        stl += "endloop\nendfacet\n";
                    }
                }
            }
        }
        let path = h.dir.join("box.stl");
        std::fs::write(&path, stl + "endsolid s\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
    }

    /// The button reads the scan and the dialog opens on what it found; a next step's button
    /// closes it and opens that tool; the survey changes nothing in the design.
    #[test]
    fn survey_reads_the_scan_and_opens_a_next_step() {
        let mut h = crate::scenario::Harness::new();
        scan(&mut h);
        let features = h.app.session.doc.features.len();
        h.app.start("scan.survey");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSurvey(_))), "the dialog is open");
        let r = kept(&h.app).expect("read");
        let next: Vec<&str> = r["next"].as_array().unwrap().iter().filter_map(|s| s["command"].as_str()).collect();
        assert_eq!(next.last(), Some(&"scan.find_all"), "{r}");
        if let Some(Kind::ScanSurvey(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.open = Some("scan.find_all".into());
        }
        tick(&mut h.app);
        assert!(!matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSurvey(_))), "the survey closed");
        assert_eq!(h.app.session.doc.features.len(), features, "nothing was built");
    }

    /// The reasons say tools by their names, not their ids; text with no id stays as it is.
    #[test]
    fn reasons_name_tools_as_people_see_them() {
        assert_eq!(named("run scan.survey again after it"), "run Survey Scan again after it");
        assert_eq!(named("thin it with scan.reduce."), "thin it with Reduce Mesh.");
        assert_eq!(named("no ids here"), "no ids here");
        assert_eq!(named("scan.nonsense stays"), "scan.nonsense stays");
    }

    /// With no scan in the design nothing opens and the status says why.
    #[test]
    fn survey_without_a_scan_opens_nothing() {
        let mut h = crate::scenario::Harness::new();
        h.app.start("scan.survey");
        assert!(!matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanSurvey(_))));
        assert!(h.app.status.as_ref().is_some_and(|s| s.2), "an error is shown");
    }
}
