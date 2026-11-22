//! Find All's regions (`scan.find_all`) in the viewport: the scan's points drawn as dots in the
//! 3D pass, each in its region's colour, through the point-cloud path as the scan deviation map
//! is, so a scan of millions of triangles stays interactive. Each shape takes an identification
//! colour, as Colored Components gives each component; freeform is the dim text colour. The
//! corner key names the largest shapes.

use std::cell::RefCell;
use std::sync::Arc;

use solidcraft_engine::cmd::scan::reconstruct::ScanShapes;
use solidcraft_engine::doc::component_color::automatic_color;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use crate::SolveApp;

/// The regions' id among the clouds the viewport draws: beside the deviation map's.
const CLOUD_ID: u64 = u64::MAX - 6;

/// A dot's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

/// The most shapes the key names.
const KEYED: usize = 8;

thread_local! {
    /// The dots last built: for which regions and freeform colour, their version, and the records.
    static BUILT: RefCell<Option<(Key, u64, Arc<Vec<u8>>)>> = const { RefCell::new(None) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
struct Key {
    shapes: Arc<ScanShapes>,
    freeform: [u8; 3],
    /// What is selected on the scan: its points are left to the selection's colour.
    selected: Option<Arc<solidcraft_engine::cmd::scan::select::ScanSelected>>,
}

impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        let same_selection = match (&self.selected, &other.selected) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        Arc::ptr_eq(&self.shapes, &other.shapes) && self.freeform == other.freeform && same_selection
    }
}

/// What Find All found, if it is shown and the scan is as it was.
pub(crate) fn current(app: &SolveApp) -> Option<Arc<ScanShapes>> {
    app.session.scan_shapes.clone().filter(|k| k.shown && k.current(&app.session))
}

/// Whether the regions are shown: the Find All button shows pressed.
pub(crate) fn shown(app: &SolveApp) -> bool {
    current(app).is_some()
}

fn freeform() -> [u8; 3] {
    let c = solidcraft_theme::Tokens::get().text_dim;
    [c.r(), c.g(), c.b()]
}

/// Region `k`'s colour: a shape's identification colour, freeform dim.
fn colour(shapes: &ScanShapes, k: u32, freeform: [u8; 3]) -> [u8; 3] {
    match shapes.outcome.segments.get(k as usize) {
        Some(segment) if segment.shape.is_some() => automatic_color(u64::from(k)),
        _ => freeform,
    }
}

/// The regions as a cloud for the GPU viewport, or nothing when none are shown.
pub(crate) fn gpu_cloud(app: &SolveApp) -> Option<GpuCloud> {
    let shapes = current(app)?;
    let selected = solidcraft_engine::cmd::scan::select::selected(&app.session).filter(|s| s.scan == shapes.scan);
    let key = Key { shapes: shapes.clone(), freeform: freeform(), selected };
    let (version, dots) = BUILT.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((k, version, dots)) = b.as_ref()
            && *k == key
        {
            return Some((*version, dots.clone()));
        }
        let dots = Arc::new(records(app, &shapes, key.freeform, key.selected.as_deref())?);
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        *b = Some((key, version, dots.clone()));
        Some((version, dots))
    })?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version,
        dots,
        segments: crate::scan_dots::none(),
        triangles: crate::scan_dots::none(),
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// One dot per point of the scan, in the colour of the first region a triangle of it is in; none
/// for the points of what is selected, so the selection's colour shows there (Find All's dots over
/// it speckled it, and Select's Shape tool needs Find All).
fn records(
    app: &SolveApp,
    shapes: &ScanShapes,
    freeform: [u8; 3],
    selected: Option<&solidcraft_engine::cmd::scan::select::ScanSelected>,
) -> Option<Vec<u8>> {
    let state = app.session.model.state();
    state.body(&shapes.scan)?;
    let shown = solidcraft_engine::cmd::scan::shown_mesh(&app.session, &shapes.scan)?;
    let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
    let mut region = vec![u32::MAX; positions.len()];
    for (t, tri) in triangles.iter().enumerate() {
        let k = shapes.regions.get(t).copied().unwrap_or(u32::MAX);
        for &v in tri {
            if let Some(slot) = region.get_mut(v as usize).filter(|r| **r == u32::MAX) {
                *slot = k;
            }
        }
    }
    let mut taken = vec![false; positions.len()];
    if let Some(sel) = selected {
        for (tri, _) in triangles.iter().zip(sel.selected.iter()).filter(|(_, s)| **s) {
            for &v in tri {
                if let Some(t) = taken.get_mut(v as usize) {
                    *t = true;
                }
            }
        }
    }
    let mut out = Vec::with_capacity(positions.len() * crate::scan_dots::DOT_SIZE);
    for ((p, &k), _) in positions.iter().zip(&region).zip(&taken).filter(|(_, t)| !**t) {
        let dot = Dot { at: [p.x as f32, p.y as f32, p.z as f32], value: k as f32, rgb: colour(shapes, k, freeform) };
        cloud_dot(&mut out, &dot);
    }
    Some(out)
}

/// The key while the regions are shown: a line of what Find All found, and the largest shapes,
/// then freeform.
pub(crate) fn legend(app: &SolveApp) -> Option<(String, Vec<(egui::Color32, String, bool)>)> {
    let shapes = current(app)?;
    let c = |rgb: [u8; 3]| crate::theme::model_color(rgb);
    let found = shapes.reading.faces.len();
    let summary = solidcraft_ui_text::format(
        "ui.scan_find_all_summary",
        &[
            ("shapes", &found.to_string()),
            ("share", &format!("{:.0} %", 100.0 * shapes.outcome.in_shapes)),
            ("noise", &crate::prefs::show_mm(app, shapes.outcome.noise, 1)),
        ],
    );
    let mut rows: Vec<(egui::Color32, String, bool)> = shapes
        .outcome
        .segments
        .iter()
        .enumerate()
        .take(found.min(KEYED))
        .filter_map(|(k, segment)| {
            let kind = match segment.shape? {
                solidcraft_engine::cmd::scan::reconstruct::Shape::Plane { .. } => solidcraft_ui_text::tr("ui.scan_find_all_plane").to_string(),
                solidcraft_engine::cmd::scan::reconstruct::Shape::Cylinder { radius, .. } => {
                    format!("{} R {}", solidcraft_ui_text::tr("ui.scan_find_all_cylinder"), crate::prefs::show_mm(app, radius, 1))
                }
                solidcraft_engine::cmd::scan::reconstruct::Shape::Sphere { radius, .. } => {
                    format!("{} R {}", solidcraft_ui_text::tr("ui.scan_find_all_sphere"), crate::prefs::show_mm(app, radius, 1))
                }
                solidcraft_engine::cmd::scan::reconstruct::Shape::Cone { half_angle, .. } => {
                    format!("{} {:.1}°", solidcraft_ui_text::tr("ui.scan_find_all_cone"), half_angle.to_degrees())
                }
                solidcraft_engine::cmd::scan::reconstruct::Shape::Torus { minor, .. } => {
                    format!("{} r {}", solidcraft_ui_text::tr("ui.scan_find_all_torus"), crate::prefs::show_mm(app, minor, 1))
                }
            };
            Some((c(colour(&shapes, k as u32, freeform())), kind, false))
        })
        .collect();
    if found > KEYED {
        rows.push((c(freeform()), solidcraft_ui_text::format("ui.scan_find_all_more", &[("count", &(found - KEYED).to_string())]), false));
    }
    if shapes.outcome.segments.len() > found {
        rows.push((c(freeform()), solidcraft_ui_text::tr("ui.scan_find_all_freeform").to_string(), false));
    }
    Some((summary, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A 30 mm cube scan, each face a 12 × 12 grid.
    fn cube(h: &mut crate::scenario::Harness) {
        let n = 12;
        let mut stl = String::from("solid b\n");
        let s = 30.0 / f64::from(n);
        // Each face of a 30 mm cube as an n × n grid of squares, two triangles each.
        for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for j in 0..n {
                for i in 0..n {
                    let at = |di: u32, dj: u32| {
                        let mut p = [0.0; 3];
                        p[axis] = if high { 30.0 } else { 0.0 };
                        p[u] = s * f64::from(i + di);
                        p[v] = s * f64::from(j + dj);
                        p
                    };
                    let quad = [at(0, 0), at(1, 0), at(1, 1), at(0, 1)];
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
        let path = h.dir.join("cube.stl");
        std::fs::write(&path, stl + "endsolid b\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
    }

    /// A finely cut box scan: Find All shows its six faces, one dot per point, keyed; pressed again
    /// the colouring goes.
    #[test]
    fn find_all_colours_the_scan_and_its_button_takes_it_away() {
        let mut h = crate::scenario::Harness::new();
        cube(&mut h);
        h.app.start("scan.find_all");
        assert!(shown(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.find_all"));
        let cloud = gpu_cloud(&h.app).unwrap();
        let points =
            h.app.session.model.state().bodies.iter().find(|b| b.body.is_mesh()).and_then(|b| b.body.triangle_mesh()).map(|(p, _)| p.len()).unwrap();
        assert_eq!(cloud.dots.len(), points * crate::scan_dots::DOT_SIZE, "one dot per point");
        let (summary, rows) = legend(&h.app).unwrap();
        assert!(summary.starts_with('6'), "{summary}");
        assert_eq!(rows.len(), 6, "{rows:?}");
        assert!(crate::viewport::analysis_legend_data(&h.app).is_some(), "the corner key");
        h.app.start("scan.find_all");
        assert!(!shown(&h.app) && gpu_cloud(&h.app).is_none(), "pressed again, the colouring goes");
    }

    /// Find All works through the whole scan (45 s on 10 million triangles): its button computes
    /// it beside the window, saying so, and the scan is coloured when it is in.
    #[test]
    fn find_all_is_computed_beside_the_window() {
        let mut h = crate::scenario::Harness::new();
        cube(&mut h);
        h.app.apply_off_thread = Some(std::time::Duration::ZERO);
        h.app.start("scan.find_all");
        assert_eq!(h.app.computing_command(), Some("scan.find_all"));
        assert!(!shown(&h.app), "nothing shown until it is in");
        for _ in 0..500 {
            if h.app.computing_command().is_none() {
                break;
            }
            h.frames(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(shown(&h.app) && legend(&h.app).is_some_and(|(summary, _)| summary.starts_with('6')));
    }
}
