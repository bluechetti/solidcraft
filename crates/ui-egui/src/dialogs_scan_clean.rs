//! The scan-cleaning dialogs. Remove Floating Pieces (`scan.remove_floating`): the scan's pieces are
//! found once, and the slider says how small a piece must be, against the largest, to float; the
//! pieces that would go are marked on the scan (through the point-cloud path, in the analysis
//! colours' bad colour) and counted. OK takes them off, one undo step.

use std::cell::Cell;
use std::sync::Arc;

use serde_json::{Value, json};
use solidcraft_engine::cmd::scan::clean;
use solidcraft_gpu::view::{CloudStyle, ColorMode, Dot, GpuCloud, cloud_dot};

use crate::SolveApp;
use crate::dialogs::{Kind, form_row};
use crate::ui_kit;

/// The marks' id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 4;

/// A mark's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    static VERSION: Cell<u64> = const { Cell::new(0) };
}

/// The Remove Floating Pieces dialog's values.
#[derive(Clone, Debug)]
pub struct Floating {
    /// The share of the largest piece below which a piece floats, as log₁₀ of a percentage.
    pub log_percent: f64,
    shown: Option<Shown>,
}

impl Default for Floating {
    fn default() -> Self {
        Floating { log_percent: (100.0 * clean::FLOATING_SHARE).log10(), shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: (u64, u64),
    result: Result<Marked, String>,
}

#[derive(Clone, Debug)]
struct Marked {
    body: String,
    pieces: usize,
    floating: usize,
    triangles: usize,
    version: u64,
    dots: Arc<Vec<u8>>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.remove_floating").then(|| (Kind::ScanFloating(Floating::default()), Vec::new()))
}

impl Floating {
    /// The share of the largest piece, 0 to 0.5.
    pub fn share(&self) -> f64 {
        (10f64.powf(self.log_percent) / 100.0).clamp(0.0, 0.5)
    }
}

/// Mark the floating pieces again when the share or the design has changed.
fn mark(app: &SolveApp, k: &mut Floating) {
    let key = (k.log_percent.to_bits(), app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        return;
    }
    let share = k.share();
    let result = clean::pieces_of(&app.session, None, "scan.remove_floating").map_err(|e| e.to_string()).and_then(|(body, found)| {
        let floating = found.floating(share);
        let state = app.session.model.state();
        let (positions, triangles) = state.body(&body).and_then(|b| b.body.triangle_mesh()).ok_or("the scan is gone")?;
        let rgb = crate::theme::Tokens::get().analysis.draft[2].map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut dots = Vec::new();
        let mut count = 0;
        for (t, tri) in triangles.iter().enumerate() {
            if !found.of.get(t).is_some_and(|&p| p != u32::MAX && floating[p as usize]) {
                continue;
            }
            count += 1;
            for &i in tri {
                if let Some(p) = positions.get(i as usize) {
                    cloud_dot(&mut dots, &Dot { at: [p.x as f32, p.y as f32, p.z as f32], rgb, ..Dot::default() });
                }
            }
        }
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        Ok(Marked {
            body,
            pieces: found.sizes.len(),
            floating: floating.iter().filter(|f| **f).count(),
            triangles: count,
            version,
            dots: Arc::new(dots),
        })
    });
    k.shown = Some(Shown { key, result });
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Floating) -> bool {
    mark(app, k);
    // 0.05 % to 50 % of the largest piece, on a log scale.
    let format = |v: f64| format!("{:.2} %", 10f64.powf(v));
    form_row(ui, crate::i18n::tr("ui.scan_floating_below"), |ui| {
        ui_kit::slider(ui, &mut k.log_percent, -1.3..=1.7, ui_kit::Slider { format, ..ui_kit::Slider::default() })
    });
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let found = crate::i18n::format(
                "ui.scan_floating_found",
                &[("floating", &m.floating.to_string()), ("pieces", &m.pieces.to_string()), ("triangles", &m.triangles.to_string())],
            );
            ui_kit::text(ui, &found, ui_kit::Text::Note("scan_floating_found"));
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_floating_error", true));
        }
        None => {}
    }
    ui_kit::text(ui, crate::i18n::tr("ui.scan_floating_hint"), ui_kit::Text::Hint("scan_floating_hint"));
    false
}

/// What OK runs: the floating pieces taken off.
pub fn commands(k: &Floating) -> Result<Vec<(String, Value)>, String> {
    let m = k.shown.as_ref().ok_or("still finding the pieces")?.result.as_ref().map_err(Clone::clone)?;
    if m.floating == 0 {
        return Err(crate::i18n::tr("ui.scan_floating_none").into());
    }
    Ok(vec![("scan.remove_floating".into(), json!({"body": m.body, "below": k.share()}))])
}

/// The floating pieces marked for the GPU viewport while the dialog is open.
pub(crate) fn gpu_marks(app: &SolveApp) -> Option<GpuCloud> {
    let Some(Kind::ScanFloating(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    let m = k.shown.as_ref()?.result.as_ref().ok()?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version: m.version,
        dots: m.dots.clone(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A box of `n` × `n` squares a side from `lo` to `hi`, as STL facets.
    fn box_facets(lo: [f64; 3], hi: [f64; 3], n: u32) -> String {
        let mut stl = String::new();
        for axis in 0..3 {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for high in [false, true] {
                let at = |i: u32, j: u32| {
                    let mut c = lo;
                    if high {
                        c[axis] = hi[axis];
                    }
                    c[u] = lo[u] + (hi[u] - lo[u]) * f64::from(i) / f64::from(n);
                    c[v] = lo[v] + (hi[v] - lo[v]) * f64::from(j) / f64::from(n);
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
        stl
    }

    /// A block with a speck beside it: the dialog marks the speck's 12 triangles and OK takes
    /// them off.
    #[test]
    fn the_speck_is_marked_and_taken_off() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("specked.stl");
        let stl =
            format!("solid s\n{}{}endsolid s\n", box_facets([0.0; 3], [30.0, 20.0, 10.0], 12), box_facets([50.0, 0.0, 0.0], [51.0, 1.0, 1.0], 1));
        std::fs::write(&path, stl).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.remove_floating");
        h.frames(2);
        let marks = gpu_marks(&h.app).expect("the speck marked");
        assert_eq!(marks.dots.len(), 12 * 3 * solidcraft_gpu::view::DOT_SIZE, "its 12 triangles' corners");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        let state = h.app.session.model.state();
        let (positions, _) = state.bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).unwrap();
        assert!(positions.iter().all(|p| p.x <= 30.0), "the speck is gone");
    }
}
