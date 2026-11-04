//! Stand on Base (`scan.stand_on_base`): the dialog finds the ways the scan could stand and marks
//! the steadiest on it, where the part would touch the ground. The Base list chooses another; a
//! face clicked on the scan is grown from the click and marked instead. OK stands the scan on it.
//!
//! The marks are the scan's points drawn as dots in the 3D pass through the point-cloud path, in
//! the highlight colour, built again only when the choice changes.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use solidcore_record::Vec3;
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::{BasePreview, base_preview};
use solidcraft_gpu::view::{CloudStyle, ColorMode, Dot, GpuCloud, cloud_dot};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::selection::{FACES, SelInput};
use crate::ui_kit::{self, choice_row};

/// The marks' id among the clouds the viewport draws (the deviation map is `u64::MAX - 1`).
const CLOUD_ID: u64 = u64::MAX - 2;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Stand on Base dialog's values (the clicked face is its selection input).
#[derive(Clone, Debug, Default)]
pub struct StandBase {
    /// The base chosen in the list, used while no face is picked.
    pub index: usize,
    /// What is marked now, and what it was worked out for.
    shown: Option<Shown>,
}

#[derive(Clone, Debug)]
struct Shown {
    key: Key,
    result: Result<Preview, String>,
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    point: Option<[u64; 3]>,
    index: usize,
    revision: u64,
}

#[derive(Clone, Debug)]
struct Preview {
    found: BasePreview,
    version: u64,
    dots: Arc<Vec<u8>>,
}

/// The dialog for Stand on Base.
pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.stand_on_base").then(|| (Kind::ScanBase(StandBase::default()), vec![SelInput::new("Face", FACES, false)]))
}

fn tr(text: &str) -> &str {
    crate::i18n::tr_label(text)
}

/// The face clicked on the scan: its scan and the point.
fn picked(inputs: &[SelInput]) -> Option<(&str, Vec3)> {
    crate::selection::items(inputs, 0).iter().find_map(|s| match s {
        Sel::Face { body, point, .. } => Some((body.as_str(), *point)),
        _ => None,
    })
}

/// Work out what to mark, when the choice or the design has changed since it last was.
fn refresh(app: &SolveApp, k: &mut StandBase, inputs: &[SelInput]) {
    let pick = picked(inputs);
    let key = Key { point: pick.map(|(_, p)| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]), index: k.index, revision: app.session.revision };
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let result = base_preview(&app.session, pick.map(|(b, _)| b), pick.map(|(_, p)| p), k.index).map_err(|e| e.to_string()).map(|found| {
        let colour = crate::theme::Tokens::get().accent;
        let rgb = [colour.r(), colour.g(), colour.b()];
        let mut dots = Vec::with_capacity(found.marked.len() * solidcraft_gpu::view::DOT_SIZE);
        for at in &found.marked {
            cloud_dot(&mut dots, &Dot { at: *at, rgb, ..Dot::default() });
        }
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Preview { found, version, dots: Arc::new(dots) }
    });
    k.shown = Some(Shown { key, result });
}

/// The dialog's own rows (after the face input); true when Enter was pressed in a value.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut StandBase, inputs: &[SelInput]) -> bool {
    refresh(app, k, inputs);
    let picked = picked(inputs).is_some();
    let Some(shown) = &k.shown else { return false };
    match &shown.result {
        Err(why) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_base_error", true));
        }
        Ok(p) => {
            let base = &p.found.base;
            if !picked && p.found.count > 0 {
                let labels: Vec<String> = (0..p.found.count).map(|i| format!("{} {}", crate::i18n::tr("ui.scan_base_number"), i + 1)).collect();
                let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                let mut index = k.index % p.found.count;
                choice_row(ui, crate::i18n::tr("ui.scan_base"), "scan_base", &mut index, &labels);
                k.index = index;
            }
            let what = match base.kind.as_str() {
                "flat" => crate::i18n::tr("ui.scan_base_flat"),
                "resting" => crate::i18n::tr("ui.scan_base_resting"),
                "axis" => crate::i18n::tr("ui.scan_base_axis"),
                _ => crate::i18n::tr("ui.scan_base_grown"),
            };
            form_row(ui, crate::i18n::tr("ui.scan_base_kind"), |ui| ui_kit::text(ui, what, ui_kit::Text::Cell));
            if base.area > 0.0 {
                let area = crate::prefs::show_mm(app, base.area, 2);
                form_row(ui, tr("Area"), |ui| ui_kit::text(ui, &area, ui_kit::Text::Cell));
            }
            if base.tip_degrees > 0.0 {
                let tip = format!("{:.0}°", base.tip_degrees);
                form_row(ui, crate::i18n::tr("ui.scan_base_tips"), |ui| ui_kit::text(ui, &tip, ui_kit::Text::Cell));
            }
            ui_kit::text(ui, crate::i18n::tr("ui.scan_base_hint"), ui_kit::Text::Hint("scan_base_hint"));
        }
    }
    false
}

/// The command the dialog's OK runs: stand on the face clicked, or on the base chosen.
pub fn commands(k: &StandBase, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let shown = k.shown.as_ref().ok_or("still finding the bases")?;
    let p = shown.result.as_ref().map_err(Clone::clone)?;
    let mut params = Map::new();
    params.insert("body".into(), json!(p.found.body));
    match picked(inputs) {
        Some((_, point)) => params.insert("point".into(), json!([point.x, point.y, point.z])),
        None => params.insert("base".into(), json!(p.found.index.unwrap_or(0))),
    };
    Ok(vec![("scan.stand_on_base".into(), Value::Object(params))])
}

/// The marks for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanBase(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let p = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: p.version,
        dots: p.dots.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solidcore_record::Vec3;
    use solidcraft_engine::geom::prelude::*;

    use super::*;

    /// A slab 80 × 50 × 10 scanned lying flat, each side cut into 8 × 8 squares, as an STL.
    fn slab_stl() -> String {
        let (size, n) = ([80.0f64, 50.0, 10.0], 8u32);
        let mut stl = String::from("solid s\n");
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for high in [false, true] {
                let at = |i: u32, j: u32| {
                    let mut c = [0.0, 0.0, 0.0];
                    if high {
                        c[axis] = size[axis];
                    }
                    c[u] = size[u] * f64::from(i) / f64::from(n);
                    c[v] = size[v] * f64::from(j) / f64::from(n);
                    c
                };
                for i in 0..n {
                    for j in 0..n {
                        for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                            stl += "facet normal 0 0 0\nouter loop\n";
                            for [x, y, z] in tri {
                                stl += &format!("vertex {x} {y} {z}\n");
                            }
                            stl += "endloop\nendfacet\n";
                        }
                    }
                }
            }
        }
        stl + "endsolid s\n"
    }

    fn bounds(h: &crate::scenario::Harness) -> (Vec3, Vec3) {
        let state = h.app.session.model.state();
        let body = state.bodies.iter().find(|b| b.body.is_mesh()).unwrap();
        let (positions, _) = body.body.triangle_mesh().unwrap();
        positions
            .iter()
            .fold((Vec3::new(f64::MAX, f64::MAX, f64::MAX), Vec3::new(f64::MIN, f64::MIN, f64::MIN)), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
    }

    /// The button opens the dialog with the steadiest base marked; the list chooses another; a face
    /// clicked on the scan is marked instead, and OK stands the slab on it, on its end.
    #[test]
    fn stand_on_base_marks_the_base_and_stands_on_the_face_clicked() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("slab.stl");
        std::fs::write(&path, slab_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.stand_on_base");
        h.frames(3);
        let marks = gpu_marks(&h.app).expect("the steadiest base is marked");
        // A broad face: 9 × 9 points of its grid.
        assert_eq!(marks.dots.len(), 81 * solidcraft_gpu::view::DOT_SIZE);

        if let Some(Kind::ScanBase(k)) = h.app.dialog.as_mut().map(|d| &mut d.kind) {
            k.index = 2;
        }
        h.frames(2);
        let other = gpu_marks(&h.app).unwrap();
        assert_ne!(other.version, marks.version, "another base, marked again");

        // The end at X = 80, clicked in the view.
        h.step(&json!({"click": {"world": [80.0, 25.0, 5.0]}})).unwrap();
        h.frames(2);
        let picked = h.app.dialog.as_ref().map(|d| d.inputs[0].items.clone()).unwrap();
        assert!(matches!(picked.first(), Some(Sel::Face { .. })), "{picked:?}");
        let end = gpu_marks(&h.app).unwrap();
        assert_eq!(end.dots.len(), 81 * solidcraft_gpu::view::DOT_SIZE, "the end face's 9 × 9 points");

        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none() && gpu_marks(&h.app).is_none());
        let (lo, hi) = bounds(&h);
        assert!(lo.z.abs() < 1e-4 && (hi.z - 80.0).abs() < 1e-4, "standing on its end: {lo:?} {hi:?}");
    }
}
