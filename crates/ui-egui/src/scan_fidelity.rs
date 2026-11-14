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

/// "passes (95 % needed)" or "fails (needs 95 % within)": the verdict every readout and key gives,
/// the same for a tool's result and for the Scan Deviation map.
pub(crate) fn verdict(within: f64) -> String {
    let share = format!("{:.0} %", 100.0 * solidcraft_engine::cmd::scan::PASS_SHARE);
    let id = if within >= solidcraft_engine::cmd::scan::PASS_SHARE { "ui.scan_deviation_pass" } else { "ui.scan_deviation_fail" };
    solidcraft_ui_text::format(id, &[("share", &share)])
}

/// A tool's tolerance as it stands: the value being tried, and the change OK keeps for the scan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tolerance {
    pub mm: f64,
    /// The scan and the tolerance to set on it when it differs from the scan's own.
    pub keep: Option<(String, f64)>,
}

/// The tolerance kept in the design for the scan, mm (the default for a scan with none, or no
/// scan yet): what commands read.
fn kept(app: &SolveApp, scan: Option<&str>) -> f64 {
    scan.map_or(solidcraft_engine::doc::scan_tolerance::DEFAULT_MM, |s| solidcraft_engine::cmd::scan::tolerance::of(&app.session, s))
}

/// The scan's tolerance, mm: the one kept for it; for a scan with none (with the preference
/// `scan_tolerance_from_noise`) three times its noise, at least the default, once the noise is
/// read; else the default.
pub(crate) fn of(app: &SolveApp, scan: Option<&str>) -> f64 {
    match noise_tolerance(app, scan) {
        Some((tolerance, _)) => tolerance,
        None => kept(app, scan),
    }
}

/// For a scan with no tolerance kept and the preference on: the tolerance its noise gives and the
/// noise, mm, once read. Read here on a small scan, beside the window on a large one (none until
/// it is in).
fn noise_tolerance(app: &SolveApp, scan: Option<&str>) -> Option<(f64, f64)> {
    let scan = scan?;
    if !app.preferences.scan_tolerance_from_noise || solidcraft_engine::cmd::scan::tolerance::is_set(&app.session, scan) {
        return None;
    }
    let noise = noise(app, scan)?;
    Some((solidcraft_engine::cmd::scan::tolerance::for_noise(noise), noise))
}

/// A scan's noise as read: none yet while it is read beside the window.
enum Reading {
    Working(crate::scan_job::Job<(), Option<f64>>),
    Read(Option<f64>),
}

thread_local! {
    /// The noise read per scan, with the mesh it was read from (a changed scan is read again).
    static NOISE: std::cell::RefCell<std::collections::HashMap<String, (usize, Reading)>> = std::cell::RefCell::default();
}

/// The noise of `scan`, mm: read once per mesh, beside the window on a large scan.
fn noise(app: &SolveApp, scan: &str) -> Option<f64> {
    let mesh = app.session.model.state().body(scan).and_then(|b| b.body.kept_mesh())?;
    let id = std::sync::Arc::as_ptr(&mesh) as usize;
    let known = NOISE.with(|n| {
        let mut n = n.borrow_mut();
        let (_, reading) = n.get_mut(scan).filter(|(at, _)| *at == id)?;
        let read = match reading {
            Reading::Read(v) => *v,
            Reading::Working(job) => match job.poll() {
                crate::scan_job::Poll::Working => return Some(None),
                crate::scan_job::Poll::Done(v) => v,
                crate::scan_job::Poll::Lost => None,
            },
        };
        *reading = Reading::Read(read);
        Some(read)
    });
    if let Some(read) = known {
        return read;
    }
    let reading = if crate::scan_job::beside_window(app) {
        let name = scan.to_owned();
        match crate::scan_job::spawn(app, (), move |s| solidcraft_engine::cmd::scan::tolerance::noise(s, &name)) {
            Some(job) => Reading::Working(job),
            None => Reading::Read(solidcraft_engine::cmd::scan::tolerance::noise(&app.session, scan)),
        }
    } else {
        Reading::Read(solidcraft_engine::cmd::scan::tolerance::noise(&app.session, scan))
    };
    let read = match &reading {
        Reading::Read(v) => *v,
        Reading::Working(_) => None,
    };
    NOISE.with(|n| {
        let mut n = n.borrow_mut();
        // A design with many scans keeps only what it reads now.
        if n.len() > 64 {
            n.clear();
        }
        n.insert(scan.to_owned(), (id, reading));
    });
    read
}

/// Whether the noise of `scan` is still being read beside the window.
fn reading(scan: Option<&str>) -> bool {
    scan.is_some_and(|s| NOISE.with(|n| matches!(n.borrow().get(s), Some((_, Reading::Working(_))))))
}

/// The tolerance in force while a tool's own value is `local` (0: the scan's).
pub(crate) fn effective(app: &SolveApp, scan: Option<&str>, local: f64) -> f64 {
    if local.is_finite() && local > 0.0 { local } else { of(app, scan) }
}

/// The Tolerance row of a tool: the scan's tolerance, or the tool's own `local` when it was changed
/// here (0 until then). The change is kept by OK for the scan.
///
/// A scan with no tolerance kept starts from three times its noise (the preference
/// `scan_tolerance_from_noise`); the row says so on hover, and OK keeps it for the scan so the
/// commands read it too.
pub(crate) fn row(app: &SolveApp, ui: &mut egui::Ui, scan: Option<&str>, local: &mut f64) -> Tolerance {
    use ui_kit::BackingTooltipExt;
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let from_noise = noise_tolerance(app, scan);
    let start = of(app, scan);
    let mut value = effective(app, scan, *local);
    let why = from_noise
        .filter(|_| *local <= 0.0)
        .map(|(_, noise)| solidcraft_ui_text::format("ui.scan_tolerance_noise", &[("noise", &crate::prefs::show_mm(app, noise, 2))]));
    form_row(ui, solidcraft_ui_text::tr("ui.scan_tolerance"), |ui| {
        let r = ui_kit::length_field(ui, &mut value, &units, decimals);
        match why {
            Some(why) => r.backing_on_hover_text(why),
            None => r,
        }
    });
    if reading(scan) {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }
    if !(value.is_finite() && value > 0.0) {
        value = start;
    }
    *local = if (value - start).abs() > 1e-12 { value } else { 0.0 };
    let changed = (value - kept(app, scan)).abs() > 1e-12;
    Tolerance { mm: value, keep: scan.filter(|_| changed).map(|s| (s.to_owned(), value)) }
}

/// What OK also runs to keep a changed tolerance for the scan.
pub(crate) fn keep(kept: Option<&(String, f64)>) -> Option<(String, Value)> {
    kept.map(|(scan, mm)| ("scan.tolerance".to_owned(), json!({"scan": scan, "tolerance": format!("{mm} mm")})))
}

/// The line every tool says about how close its result is.
pub(crate) fn line(app: &SolveApp, s: &Stats, tolerance: f64) -> String {
    let mm = |v: f64| crate::prefs::show_mm(app, v, 1);
    let said = solidcraft_ui_text::format(
        "ui.scan_fidelity",
        &[
            ("rms", &mm(s.rms)),
            ("mean", &mm(s.mean)),
            ("above", &mm(s.max_above)),
            ("below", &mm(s.max_below.abs())),
            ("within", &format!("{:.0} %", 100.0 * s.within)),
            ("tolerance", &mm(tolerance)),
        ],
    );
    format!("{said} · {}", verdict(s.within))
}

/// A tool's Fit row: the share within the tolerance (the row above it), red when it fails; the
/// whole line (RMS, mean, worst each way, the verdict) on hover, with what the tool `also` says
/// about its result.
pub(crate) fn readout(app: &SolveApp, ui: &mut egui::Ui, id: &'static str, s: &Stats, tolerance: f64, also: Option<&str>) {
    use ui_kit::BackingTooltipExt;
    let short = solidcraft_ui_text::format("ui.scan_fit_within", &[("within", &format!("{:.0} %", 100.0 * s.within))]);
    let fails = s.within < solidcraft_engine::cmd::scan::PASS_SHARE;
    let full = match also {
        Some(also) => format!("{also}\n{}", line(app, s, tolerance)),
        None => line(app, s, tolerance),
    };
    form_row(ui, solidcraft_ui_text::tr("ui.scan_fit_result"), |ui| {
        ui_kit::text(ui, &short, ui_kit::Text::Report(id, fails)).backing_on_hover_text(full)
    });
}

/// The key rows, top to bottom: far above, above, within, below and far below the tolerance, in
/// the Scan Deviation colours; `deep` is where the colour is at its deepest, mm.
pub(crate) fn key_rows(app: &SolveApp, tolerance: f64, deep: f64) -> Vec<(Color32, String, bool)> {
    let colours = crate::scan_deviation::colours();
    let tol = crate::prefs::show_mm(app, tolerance, 1);
    let deep = crate::prefs::show_mm(app, deep, 1);
    let scale = |s: f64| crate::theme::model_color(crate::scan_deviation::colour(colours, s));
    vec![
        (scale(1.0), format!("≥ +{deep}"), false),
        (scale(0.25), format!("+{tol} … +{deep}"), false),
        (scale(0.0), format!("± {tol}"), false),
        (scale(-0.25), format!("−{deep} … −{tol}"), false),
        (scale(-1.0), format!("≤ −{deep}"), false),
    ]
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
    let summary = solidcraft_ui_text::format(
        "ui.scan_deviation_summary",
        &[
            ("rms", &crate::prefs::show_mm(app, s.rms, 1)),
            ("within", &format!("{:.0} %", 100.0 * s.within)),
            ("tolerance", &crate::prefs::show_mm(app, tolerance, 1)),
        ],
    );
    Some((format!("{summary} · {}", verdict(s.within)), key_rows(app, tolerance, tolerance * crate::dialogs_scan_surface::DEEPEST)))
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
