//! One answer to "is this still faithful to the scan?" for every Scan-tab tool. Free Form, Edit
//! Surface, Fit Patches and Flat Template colour the scan under their result by how far it is from
//! it (the Scan Deviation colours and key), and say the same line: root mean square, mean, worst
//! above and below, and the share within the tolerance. The tolerance is the scan's own, kept in
//! the design (`scan.tolerance`) and set once: a tool's Tolerance row shows it and a change there
//! is kept by OK, so the next tool opens with it.

use egui::Color32;
use serde_json::{Value, json};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit;

/// What the distances of a scan's points from a result add up to, in mm (above the result
/// positive).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Stats {
    pub rms: f64,
    pub mean: f64,
    pub max_above: f64,
    pub max_below: f64,
    /// The share of the points within the tolerance, 0 to 1.
    pub within: f64,
    pub count: usize,
}

/// The distances `values` (NaN where a point was not measured) add up to against `tolerance`; none
/// when no point was measured.
pub(crate) fn stats(values: &[f32], tolerance: f64) -> Option<Stats> {
    let measured: Vec<f64> = values.iter().filter(|v| v.is_finite()).map(|&v| f64::from(v)).collect();
    if measured.is_empty() {
        return None;
    }
    let n = measured.len() as f64;
    Some(Stats {
        rms: (measured.iter().map(|v| v * v).sum::<f64>() / n).sqrt(),
        mean: measured.iter().sum::<f64>() / n,
        max_above: measured.iter().copied().fold(0.0, f64::max),
        max_below: measured.iter().copied().fold(0.0, f64::min),
        within: measured.iter().filter(|v| v.abs() <= tolerance).count() as f64 / n,
        count: measured.len(),
    })
}

/// A tool's tolerance as it stands: the value being tried, and the change OK keeps for the scan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tolerance {
    pub mm: f64,
    /// The scan and the tolerance to set on it when it differs from the scan's own.
    pub keep: Option<(String, f64)>,
}

/// The scan's own tolerance, mm (the default for a scan with none, or no scan yet).
pub(crate) fn of(app: &SolveApp, scan: Option<&str>) -> f64 {
    scan.map_or(solidcraft_engine::doc::scan_tolerance::DEFAULT_MM, |s| solidcraft_engine::cmd::scan::tolerance::of(&app.session, s))
}

/// The tolerance in force while a tool's own value is `local` (0: the scan's).
pub(crate) fn effective(app: &SolveApp, scan: Option<&str>, local: f64) -> f64 {
    if local.is_finite() && local > 0.0 { local } else { of(app, scan) }
}

/// The Tolerance row of a tool: the scan's tolerance, or the tool's own `local` when it was changed
/// here (0 until then). The change is kept by OK for the scan.
pub(crate) fn row(app: &SolveApp, ui: &mut egui::Ui, scan: Option<&str>, local: &mut f64) -> Tolerance {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let stored = of(app, scan);
    let mut value = effective(app, scan, *local);
    form_row(ui, crate::i18n::tr("ui.scan_tolerance"), |ui| ui_kit::length_field(ui, &mut value, &units, decimals));
    if !(value.is_finite() && value > 0.0) {
        value = stored;
    }
    *local = if (value - stored).abs() > 1e-12 { value } else { 0.0 };
    Tolerance { mm: value, keep: scan.filter(|_| *local > 0.0).map(|s| (s.to_owned(), value)) }
}

/// What OK also runs to keep a changed tolerance for the scan.
pub(crate) fn keep(kept: Option<&(String, f64)>) -> Option<(String, Value)> {
    kept.map(|(scan, mm)| ("scan.tolerance".to_owned(), json!({"scan": scan, "tolerance": format!("{mm} mm")})))
}

/// The line every tool says about how close its result is.
pub(crate) fn line(app: &SolveApp, s: &Stats, tolerance: f64) -> String {
    let mm = |v: f64| crate::prefs::show_mm(app, v, 1);
    crate::i18n::format(
        "ui.scan_fidelity",
        &[
            ("rms", &mm(s.rms)),
            ("mean", &mm(s.mean)),
            ("above", &mm(s.max_above)),
            ("below", &mm(s.max_below.abs())),
            ("within", &format!("{:.0} %", 100.0 * s.within)),
            ("tolerance", &mm(tolerance)),
        ],
    )
}

/// The line as a note under a tool's rows.
pub(crate) fn readout(app: &SolveApp, ui: &mut egui::Ui, id: &'static str, s: &Stats, tolerance: f64) {
    ui_kit::text(ui, &line(app, s, tolerance), ui_kit::Text::Note(id));
}

/// The key rows: above, within and below the tolerance, in the Scan Deviation colours.
pub(crate) fn key_rows(app: &SolveApp, tolerance: f64) -> Vec<(Color32, String, bool)> {
    let [below, within, above] = crate::scan_deviation::colours();
    let c = |rgb: [u8; 3]| crate::theme::model_color(rgb);
    let tol = crate::prefs::show_mm(app, tolerance, 1);
    let scale = |s: f64| crate::scan_deviation::colour([below, within, above], s);
    vec![(c(scale(1.0)), format!("> +{tol}"), false), (c(within), format!("± {tol}"), false), (c(scale(-1.0)), format!("< −{tol}"), false)]
}

/// The key and summary of the colouring the open tool shows, for the viewport's corner key.
pub(crate) fn legend(app: &SolveApp) -> Option<(String, Vec<(Color32, String, bool)>)> {
    let (s, tolerance) = match app.dialog.as_ref().map(|d| &d.kind)? {
        Kind::ScanFreeForm(k) => crate::dialogs_scan_free_form::fidelity(app, k)?,
        Kind::ScanEditSurface(k) => crate::dialogs_scan_surface::fidelity(app, k)?,
        Kind::ScanFitPatches(k) => crate::dialogs_scan_patches::fidelity(app, k)?,
        Kind::ScanFlatTemplate(k) => crate::dialogs_scan_template::fidelity(app, k)?,
        _ => return None,
    };
    let summary = crate::i18n::format(
        "ui.scan_deviation_summary",
        &[
            ("rms", &crate::prefs::show_mm(app, s.rms, 1)),
            ("within", &format!("{:.0} %", 100.0 * s.within)),
            ("tolerance", &crate::prefs::show_mm(app, tolerance, 1)),
        ],
    );
    Some((summary, key_rows(app, tolerance)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers: signed mean, the largest each way, the share within, and NaN left out.
    #[test]
    fn the_distances_add_up_the_way_scan_deviation_does() {
        let s = stats(&[0.05, -0.05, 0.3, f32::NAN, -0.09], 0.1).unwrap();
        assert_eq!(s.count, 4);
        assert!((s.max_above - 0.3).abs() < 1e-6 && (s.max_below + 0.09).abs() < 1e-6, "{s:?}");
        assert!((s.mean - 0.0525).abs() < 1e-6, "{}", s.mean);
        assert!((s.within - 0.75).abs() < 1e-9, "0.3 alone is out of ±0.1: {}", s.within);
        assert!(stats(&[f32::NAN], 0.1).is_none() && stats(&[], 0.1).is_none());
    }
}
