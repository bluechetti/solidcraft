//! Unseen Surface (`scan.unseen`) in the view: the button paints the triangles of the scan its
//! scanner never saw (filled, mirrored, made from sparse points) in the warning colour and says
//! how much of the scan they are; pressing it again takes the paint away. It follows the scan:
//! a step edited or the scan moved, it is read again.

use std::cell::RefCell;
use std::sync::{Arc, Weak};

use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::SolveApp;

/// The paint's id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 31;

#[derive(Clone)]
struct Shown {
    body: String,
    /// The scan as it shows when it was read: read again when it is another.
    mesh: Weak<solidcraft_engine::geom::Mesh>,
    triangles: Arc<Vec<u8>>,
    version: u64,
}

thread_local! {
    static STATE: RefCell<Option<Shown>> = const { RefCell::new(None) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Whether the paint is shown (the button shows pressed).
pub(crate) fn shown(_app: &SolveApp) -> bool {
    STATE.with(|s| s.borrow().is_some())
}

/// The button: paints the scan's unseen surface, or takes the paint away. False: the button runs
/// nothing more.
pub(crate) fn toggle(app: &mut SolveApp) -> bool {
    if shown(app) {
        STATE.with(|s| *s.borrow_mut() = None);
        return false;
    }
    match solidcraft_engine::cmd::scan::mesh_body_name(&app.session, None, "scan.unseen") {
        Ok(body) => {
            let (shown, message) = read(app, &body);
            STATE.with(|s| *s.borrow_mut() = shown);
            app.set_status(message, false);
        }
        Err(e) => app.set_status(e.to_string(), true),
    }
    false
}

/// The scan `body` read and painted, and what to say of it.
fn read(app: &SolveApp, body: &str) -> (Option<Shown>, String) {
    let s = &app.session;
    let Some(mesh) = solidcraft_engine::cmd::scan::shown_mesh(s, body) else {
        return (None, solidcraft_ui_text::format("ui.scan_unseen_none", &[("scan", body)]));
    };
    let found = solidcraft_engine::cmd::scan::unseen::unseen_of(s, body).ok().and_then(|(_, u)| u).filter(|u| u.area > 0.0);
    let Some(found) = found else { return (None, solidcraft_ui_text::format("ui.scan_unseen_none", &[("scan", body)])) };
    let c = solidcraft_theme::Tokens::get().warning;
    let rgb = [c.r(), c.g(), c.b()];
    let lifts = crate::scan_dots::PaintLifts::of(&mesh.positions, &mesh.triangles);
    let mut out = Vec::new();
    let unseen = mesh.triangles.iter().zip(&found.flags).filter(|(_, f)| **f).map(|(tri, _)| *tri);
    for tri in unseen.take(crate::scan_dots::PAINT_MOST) {
        lifts.paint(&mut out, &mesh.positions, tri, 0.0, rgb);
    }
    let version = VERSION.with(|v| {
        v.set(v.get() + 1);
        v.get()
    });
    let message = solidcraft_ui_text::format(
        "ui.scan_unseen_found",
        &[("share", &format!("{:.1}", 100.0 * found.share())), ("area", &crate::prefs::show_mm(app, found.area, 0)), ("scan", body)],
    );
    (Some(Shown { body: body.to_owned(), mesh: Arc::downgrade(&mesh), triangles: Arc::new(out), version }), message)
}

/// The paint, for the GPU viewport: read again when the scan it was read on is no longer the one
/// shown.
pub(crate) fn gpu_cloud(app: &SolveApp) -> Option<GpuCloud> {
    let shown = STATE.with(|s| s.borrow().clone())?;
    let current = solidcraft_engine::cmd::scan::shown_mesh(&app.session, &shown.body);
    let shown = match current {
        Some(m) if shown.mesh.upgrade().is_some_and(|k| Arc::ptr_eq(&k, &m)) => shown,
        Some(_) => {
            let (again, _) = read(app, &shown.body);
            STATE.with(|s| *s.borrow_mut() = again.clone());
            again?
        }
        None => {
            STATE.with(|s| *s.borrow_mut() = None);
            return None;
        }
    };
    Some(GpuCloud {
        id: CLOUD_ID,
        version: shown.version,
        dots: crate::scan_dots::none(),
        segments: crate::scan_dots::none(),
        triangles: shown.triangles,
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: 3.0 },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A sheet with a hole filled: the button paints the patch and says so; again, it takes it away.
    #[test]
    fn the_button_paints_a_filled_hole_and_takes_it_away() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("holed.obj");
        let mut obj = String::new();
        let n = 12;
        for j in 0..=n {
            for i in 0..=n {
                obj += &format!("v {i} {j} 0\n");
            }
        }
        let at = |i: usize, j: usize| j * (n + 1) + i + 1;
        for j in 0..n {
            for i in 0..n {
                if (4..8).contains(&i) && (4..8).contains(&j) {
                    continue;
                }
                obj += &format!("f {} {} {}\nf {} {} {}\n", at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j), at(i + 1, j + 1), at(i, j + 1));
            }
        }
        std::fs::write(&path, obj).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.unseen");
        assert!(!shown(&h.app), "as scanned: nothing to paint");
        h.app.run("scan.fill_holes", json!({})).unwrap();
        h.app.start("scan.unseen");
        assert!(shown(&h.app) && crate::toolbar::shows_pressed(&h.app, "scan.unseen"));
        let cloud = gpu_cloud(&h.app).expect("the patch painted");
        assert!(!cloud.triangles.is_empty());
        h.app.start("scan.unseen");
        assert!(!shown(&h.app) && gpu_cloud(&h.app).is_none(), "pressed again, the paint goes");
    }
}
