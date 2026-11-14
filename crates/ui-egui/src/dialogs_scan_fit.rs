//! Fit Shape (`scan.fit_plane`, `_cylinder`, `_sphere`, `_cone`, `_torus`): one dialog for all five.
//! Click the scan where the shape is; the region the shape was fitted to is painted on the scan,
//! and the dialog says the shape's size and how closely it fits, and what OK keeps (a construction
//! plane, axis or point). The shape can be changed without clicking again. On a large scan the fit
//! is worked out beside the window, the last one staying painted until the new one arrives.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::fitting::{Kind as Shape, Shape as Fitted};
use solidcraft_engine::cmd::scan::{FitPreview, fit_preview};
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::scan_job;
use crate::selection::{FACES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The painted region's id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 21;

/// The commands, in the dialog's order of shapes.
pub const COMMANDS: [&str; 5] = ["scan.fit_plane", "scan.fit_cylinder", "scan.fit_sphere", "scan.fit_cone", "scan.fit_torus"];
const SHAPES: [Shape; 5] = [Shape::Plane, Shape::Cylinder, Shape::Sphere, Shape::Cone, Shape::Torus];

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Fit Shape dialog's values (the click on the scan is its selection input).
#[derive(Clone, Debug, Default)]
pub struct FitShape {
    /// The shape fitted (index into [`COMMANDS`]).
    pub shape: usize,
    shown: Option<Box<Shown>>,
    pending: Option<Arc<scan_job::Job<String, Result<Painted, String>>>>,
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<Painted, String>,
}

#[derive(Clone, Debug)]
struct Painted {
    found: FitPreview,
    version: u64,
    triangles: Arc<Vec<u8>>,
}

pub fn start(app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    let shape = COMMANDS.iter().position(|c| *c == id)?;
    let mut input = SelInput::new("Point", FACES, false);
    // A point picked on a scan before the button keeps working, as the command takes it.
    if let Some(pick) = app.session.selection.iter().rev().find(|s| matches!(s, Sel::Face { body, .. } if is_scan(app, body))) {
        input.items.push(pick.clone());
    }
    Some((Kind::ScanFit(FitShape { shape, ..FitShape::default() }), vec![input]))
}

fn is_scan(app: &SolveApp, body: &str) -> bool {
    app.session.model.state().body(body).is_some_and(|b| b.body.is_mesh())
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The click on the scan: its scan and the point.
fn picked(app: &SolveApp, inputs: &[SelInput]) -> Option<(String, Vec3)> {
    items(inputs, 0).iter().find_map(|s| match s {
        Sel::Face { body, point, .. } if is_scan(app, body) => Some((body.clone(), *point)),
        _ => None,
    })
}

fn compute(s: &solidcraft_engine::Session, body: &str, point: Vec3, shape: Shape) -> Result<Painted, String> {
    let found = fit_preview(s, Some(body), point, shape).map_err(|e| e.to_string())?;
    let rgb = solidcraft_theme::Tokens::get().analysis.draft[1].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    let mut out = Vec::with_capacity(found.region.len() * crate::scan_dots::DOT_SIZE);
    for &corners in &found.region {
        crate::scan_dots::cloud_painted(&mut out, corners, 0.0, rgb);
    }
    let version = VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    });
    Ok(Painted { found, version, triangles: Arc::new(out) })
}

/// Fit again when the click, the shape or the design has changed.
fn look(app: &SolveApp, k: &mut FitShape, inputs: &[SelInput]) {
    let Some((body, point)) = picked(app, inputs) else {
        k.shown = None;
        k.pending = None;
        return;
    };
    let shape = SHAPES[k.shape.min(SHAPES.len() - 1)];
    let key = format!("{body}|{:?}|{:?}|{}", point, shape, app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        k.pending = None;
        return;
    }
    if scan_job::beside_window(app) {
        if let Some(job) = k.pending.clone() {
            match job.poll() {
                scan_job::Poll::Working if job.key == key => return,
                scan_job::Poll::Done(result) if job.key == key => {
                    k.pending = None;
                    k.shown = Some(Box::new(Shown { key, result }));
                    return;
                }
                _ => k.pending = None,
            }
        }
        let given = body.clone();
        if let Some(job) = scan_job::spawn(app, key.clone(), move |s| compute(s, &given, point, shape)) {
            k.pending = Some(Arc::new(job));
            return;
        }
    }
    let result = compute(&app.session, &body, point, shape);
    k.shown = Some(Box::new(Shown { key, result }));
}

/// A direction as three short numbers.
fn direction(v: Vec3) -> String {
    format!("{:.3}, {:.3}, {:.3}", v.x, v.y, v.z)
}

/// The dialog's rows (after the click).
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FitShape, inputs: &[SelInput]) -> bool {
    let shapes =
        [tr("ui.scan_split_plane"), tr("ui.scan_split_cylinder"), tr("ui.scan_split_sphere"), tr("ui.scan_split_cone"), tr("ui.scan_split_torus")];
    choice_row(ui, solidcraft_ui_text::tr_label("Shape"), "scan_fit_shape", &mut k.shape, &shapes);
    look(app, k, inputs);
    let len = |v: f64| crate::prefs::show_mm(app, v, 1);
    let row = |ui: &mut egui::Ui, label: &str, text: &str| form_row(ui, label, |ui| ui_kit::text(ui, text, ui_kit::Text::Cell));
    let label = solidcraft_ui_text::tr_label;
    match k.shown.as_ref().map(|s| &s.result) {
        None => {
            ui_kit::text(ui, tr("ui.scan_fit_click"), ui_kit::Text::Hint("scan_fit_click"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_fit_error", true));
        }
        Some(Ok(p)) => {
            let f = &p.found.fitted;
            let makes = match f.shape {
                Fitted::Plane { normal, .. } => {
                    row(ui, label("Direction"), &direction(normal));
                    "ui.scan_fit_makes_plane"
                }
                Fitted::Cylinder { axis, radius, .. } => {
                    row(ui, label("Diameter"), &len(2.0 * radius));
                    row(ui, label("Length"), &len(2.0 * f.half_length));
                    row(ui, label("Direction"), &direction(axis));
                    "ui.scan_fit_makes_axis"
                }
                Fitted::Sphere { radius, .. } => {
                    row(ui, label("Diameter"), &len(2.0 * radius));
                    "ui.scan_fit_makes_point"
                }
                Fitted::Cone { axis, half_angle, .. } => {
                    row(ui, tr("ui.scan_fit_half_angle"), &format!("{:.2}°", half_angle.to_degrees()));
                    row(ui, label("Direction"), &direction(axis));
                    "ui.scan_fit_makes_axis"
                }
                Fitted::Torus { axis, major, minor, .. } => {
                    row(ui, label("Radius"), &len(major));
                    row(ui, tr("ui.scan_fit_tube"), &len(minor));
                    row(ui, label("Direction"), &direction(axis));
                    "ui.scan_fit_makes_axis"
                }
            };
            row(ui, label("Area"), &crate::prefs::show_mm(app, f.area, 2));
            let fit = solidcraft_ui_text::format("ui.scan_fit_off", &[("rms", &len(f.rms)), ("max", &len(f.max))]);
            row(ui, tr("ui.scan_fit_result"), &fit);
            ui_kit::text(ui, tr(makes), ui_kit::Text::Hint("scan_fit_makes"));
        }
    }
    if k.pending.is_some() {
        scan_job::refining_note(ui, "scan_fit_refining");
    }
    false
}

/// What OK runs: the shape kept where it was fitted.
pub fn commands(k: &FitShape, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = k.shown.as_ref().ok_or_else(|| tr("ui.scan_fit_click").to_owned())?.result.as_ref().map_err(Clone::clone)?;
    let point = items(inputs, 0)
        .iter()
        .find_map(|s| match s {
            Sel::Face { body, point, .. } if *body == p.found.body => Some(*point),
            _ => None,
        })
        .ok_or_else(|| tr("ui.scan_fit_click").to_owned())?;
    Ok(vec![(COMMANDS[k.shape.min(COMMANDS.len() - 1)].into(), json!({"body": p.found.body, "point": [point.x, point.y, point.z]}))])
}

/// The fitted region painted on the scan while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanFit(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let p = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: p.version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: p.triangles.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: 3.0 },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solidcore_record::Vec3;
    use solidcraft_engine::Sel;

    use crate::dialogs::Kind;

    /// A tube: the Fit dialog opened from Fit Plane, its shape turned to Cylinder, a click on the
    /// wall paints the region and reads the diameter; OK keeps an axis.
    #[test]
    fn a_click_on_a_tube_fits_a_cylinder_and_ok_keeps_its_axis() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("tube.obj");
        let (r, n, rings) = (5.0_f64, 48, 12);
        let mut obj = String::new();
        for j in 0..=rings {
            for i in 0..n {
                let a = std::f64::consts::TAU * f64::from(i) / f64::from(n);
                obj.push_str(&format!("v {} {} {}\n", r * a.cos(), r * a.sin(), 2.0 * f64::from(j)));
            }
        }
        for j in 0..rings {
            for i in 0..n {
                let (a, b) = (j * n + i + 1, j * n + (i + 1) % n + 1);
                let (c, d) = (a + n, b + n);
                obj.push_str(&format!("f {a} {b} {d}\nf {a} {d} {c}\n"));
            }
        }
        std::fs::write(&path, obj).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let body = h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).map(|b| b.name.clone()).unwrap();
        h.app.start("scan.fit_plane");
        let Some(d) = h.app.dialog.as_mut() else { panic!("the Fit dialog opens") };
        let Kind::ScanFit(k) = &mut d.kind else { panic!("the Fit dialog") };
        k.shape = 1;
        d.inputs[0].items.push(Sel::Face { body: body.clone(), index: 0, point: Vec3::new(r, 0.0, 12.0) });
        h.frames(3);
        let Some(Kind::ScanFit(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("still open") };
        let Some(Ok(p)) = k.shown.as_ref().map(|s| &s.result) else { panic!("fitted: {:?}", k.shown.as_ref().map(|s| &s.result)) };
        let solidcraft_engine::cmd::scan::fitting::Shape::Cylinder { radius, .. } = p.found.fitted.shape else { panic!("a cylinder") };
        assert!((radius - r).abs() < 0.1, "radius {radius}");
        assert!(!p.triangles.is_empty(), "the region is painted");
        assert!(super::gpu_marks(&h.app).is_some());
        let inputs = h.app.dialog.as_ref().map(|d| d.inputs.clone()).unwrap();
        let cmds = super::commands(k, &inputs).unwrap();
        assert_eq!(cmds[0].0, "scan.fit_cylinder");
        let axes = h.app.session.doc.features.len();
        h.app.run(&cmds[0].0, cmds[0].1.clone()).unwrap();
        assert!(h.app.session.doc.features.len() > axes, "the axis is kept");
    }
}
