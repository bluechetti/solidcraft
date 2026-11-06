//! The scan deviation map (`scan.deviation`) in the viewport: the scan's points drawn as dots in
//! the 3D pass, depth-tested with the model, each coloured by its distance from the model. The
//! dots go through the point-cloud path, so they are uploaded once per map and cost nothing to
//! draw again when the pointer moves; a scan of millions of triangles stays interactive.
//!
//! The colours are the theme's analysis colours, so they follow the colour vision setting: within
//! the tolerance the draft analysis's good colour, above the model its bad colour and below it
//! the curvature map's deep end, deepening to the limit.

use std::cell::RefCell;
use std::sync::Arc;

use solidcraft_engine::cmd::scan::DeviationMap;
use solidcraft_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use crate::SolveApp;

/// The map's id among the clouds the viewport draws: far above any point cloud's.
const CLOUD_ID: u64 = u64::MAX - 1;

/// A dot's size on screen, in pixels: dense enough on a scan to read as a coloured surface.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    /// The dots last built: for which map and colours, their version, and the records.
    static BUILT: RefCell<Option<(Key, u64, Arc<Vec<u8>>)>> = const { RefCell::new(None) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[derive(Clone)]
struct Key {
    map: Arc<DeviationMap>,
    colours: [[u8; 3]; 3],
}

/// The same map (by identity: comparing millions of values every frame would cost more than
/// drawing them) in the same colours.
impl PartialEq for Key {
    fn eq(&self, other: &Key) -> bool {
        Arc::ptr_eq(&self.map, &other.map) && self.colours == other.colours
    }
}

/// The map, if one is shown and still stands for the design.
pub(crate) fn current(app: &SolveApp) -> Option<Arc<DeviationMap>> {
    app.session.scan_deviation.clone().filter(|m| m.current(&app.session))
}

/// Whether the map is shown: the Scan Deviation button shows pressed.
pub(crate) fn shown(app: &SolveApp) -> bool {
    current(app).is_some()
}

/// The three colours of the scale: below, within, above.
pub(crate) fn colours() -> [[u8; 3]; 3] {
    let a = crate::theme::Tokens::get().analysis;
    let byte = |c: [f32; 3]| c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
    [byte(a.curvature[0]), byte(a.draft[0]), byte(a.draft[2])]
}

/// A point's colour at `s` on the scale (-1 to 1, 0 within the tolerance): out of the tolerance it
/// starts a third of the way to the far colour, so it never reads as within.
pub(crate) fn colour([below, within, above]: [[u8; 3]; 3], s: f64) -> [u8; 3] {
    if s == 0.0 {
        return within;
    }
    let far = if s > 0.0 { above } else { below };
    let k = 0.35 + 0.65 * s.abs().min(1.0);
    std::array::from_fn(|i| (f64::from(within[i]) + (f64::from(far[i]) - f64::from(within[i])) * k).round() as u8)
}

/// The map as a cloud for the GPU viewport, or nothing when none is shown.
pub(crate) fn gpu_cloud(app: &SolveApp) -> Option<GpuCloud> {
    let map = current(app)?;
    let key = Key { map: map.clone(), colours: colours() };
    let (version, dots) = BUILT.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((k, version, dots)) = b.as_ref()
            && *k == key
        {
            return Some((*version, dots.clone()));
        }
        let dots = Arc::new(records(app, &map, key.colours)?);
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

/// One dot per measured point of the scan; points the model leaves out are not drawn.
fn records(app: &SolveApp, map: &DeviationMap, colours: [[u8; 3]; 3]) -> Option<Vec<u8>> {
    let state = app.session.model.state();
    let (positions, _) = state.body(&map.scan)?.body.triangle_mesh()?;
    let mut out = Vec::with_capacity(positions.len() * crate::scan_dots::DOT_SIZE);
    for (p, &v) in positions.iter().zip(&map.values) {
        let Some(s) = map.scale(v) else { continue };
        let dot = Dot { at: [p.x as f32, p.y as f32, p.z as f32], value: v, rgb: colour(colours, s) };
        cloud_dot(&mut out, &dot);
    }
    Some(out)
}

/// The legend's rows while the map is shown: above, within and below the tolerance, and a line of
/// what it adds up to.
pub(crate) fn legend(app: &SolveApp) -> Option<(String, Vec<(egui::Color32, String, bool)>)> {
    let map = current(app)?;
    let [below, within, above] = colours();
    let c = |rgb: [u8; 3]| crate::theme::model_color(rgb);
    let tol = crate::prefs::show_mm(app, map.tolerance, 1);
    let summary = crate::i18n::format(
        "ui.scan_deviation_summary",
        &[("rms", &crate::prefs::show_mm(app, map.rms, 1)), ("within", &format!("{:.0} %", 100.0 * map.within)), ("tolerance", &tol)],
    );
    let rows = vec![
        (c(colour([below, within, above], 1.0)), format!("> +{tol}"), false),
        (c(within), format!("± {tol}"), false),
        (c(colour([below, within, above], -1.0)), format!("< −{tol}"), false),
    ];
    Some((summary, rows))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A scan of a 10 mm cube measured against a solid 10 mm cube: every point is on it, the map is
    /// drawn as one dot per point, the key reads in millimetres, and the button takes it away again.
    #[test]
    fn the_map_is_drawn_keyed_and_taken_away_by_its_button() {
        let mut h = crate::scenario::Harness::new();
        let mut stl = String::from("solid c\n");
        let corner = |i: usize| [if i & 1 == 0 { 0 } else { 10 }, if i & 2 == 0 { 0 } else { 10 }, if i & 4 == 0 { 0 } else { 10 }];
        for f in [[0, 2, 1], [1, 2, 3], [4, 5, 6], [5, 7, 6], [0, 1, 4], [1, 5, 4], [2, 6, 3], [3, 6, 7], [0, 4, 2], [2, 4, 6], [1, 3, 5], [3, 7, 5]]
        {
            stl += "facet normal 0 0 0\nouter loop\n";
            for i in f {
                let [x, y, z] = corner(i);
                stl += &format!("vertex {x} {y} {z}\n");
            }
            stl += "endloop\nendfacet\n";
        }
        let path = h.dir.join("cube.stl");
        std::fs::write(&path, stl + "endsolid c\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 10, "width": 10, "height": 10})).unwrap();
        h.app.run("scan.deviation", json!({"body": "Model"})).unwrap();
        assert!(shown(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.deviation"));
        let cloud = gpu_cloud(&h.app).unwrap();
        assert_eq!(cloud.dots.len(), 8 * crate::scan_dots::DOT_SIZE, "one dot per corner of the cube");
        let (summary, rows) = legend(&h.app).unwrap();
        assert!(summary.contains("mm") && !summary.contains("mm²") && !summary.contains("mm³"), "{summary}");
        assert_eq!(rows.len(), 3);
        assert!(crate::viewport::analysis_legend_data(&h.app).is_some(), "the corner key");

        h.app.start("scan.deviation");
        assert!(!shown(&h.app) && gpu_cloud(&h.app).is_none(), "pressed again, the map goes");
    }

    #[test]
    fn the_scale_runs_from_below_through_within_to_above() {
        let c = [[0, 0, 200], [0, 200, 0], [200, 0, 0]];
        assert_eq!(colour(c, 0.0), [0, 200, 0]);
        assert_eq!(colour(c, 1.0), [200, 0, 0]);
        assert_eq!(colour(c, -1.0), [0, 0, 200]);
        let just_above = colour(c, 0.01);
        assert!(just_above[0] > 60 && just_above[1] < 140, "out of the tolerance never reads as within: {just_above:?}");
    }
}
