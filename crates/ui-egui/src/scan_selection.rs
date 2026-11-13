//! What is selected on a scan (`scan.select`) in the viewport: the selected triangles' points
//! drawn as dots in the selection colour in the 3D pass, through the point-cloud path Find All's
//! colouring takes, so a selection of millions of triangles stays interactive.

use std::cell::RefCell;
use std::sync::Arc;

use solidcraft_engine::cmd::scan::select::{self, ScanSelected};
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::scan_dots::{Dot, cloud_dot};

use crate::SolveApp;

/// The selection's id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 7;

/// A dot's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

thread_local! {
    /// The dots last built: for which selection and colour, their version, and the records.
    static BUILT: RefCell<Option<(Arc<ScanSelected>, [u8; 3], u64, Arc<Vec<u8>>)>> = const { RefCell::new(None) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// What is selected on a scan as it is now.
pub(crate) fn current(app: &SolveApp) -> Option<Arc<ScanSelected>> {
    select::selected(&app.session)
}

fn colour() -> [u8; 3] {
    let c = solidcraft_theme::Tokens::get().sel_face;
    [c.r(), c.g(), c.b()]
}

/// The selection as a cloud for the GPU viewport, or nothing when nothing is selected.
pub(crate) fn gpu_cloud(app: &SolveApp) -> Option<GpuCloud> {
    let selected = current(app)?;
    let rgb = colour();
    let (version, dots) = BUILT.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((k, c, version, dots)) = b.as_ref()
            && Arc::ptr_eq(k, &selected)
            && *c == rgb
        {
            return Some((*version, dots.clone()));
        }
        let dots = Arc::new(records(app, &selected, rgb)?);
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        *b = Some((selected.clone(), rgb, version, dots.clone()));
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

/// One dot per point of a selected triangle.
fn records(app: &SolveApp, selected: &ScanSelected, rgb: [u8; 3]) -> Option<Vec<u8>> {
    let state = app.session.model.state();
    let (positions, triangles) = state.body(&selected.scan)?.body.triangle_mesh()?;
    let mut on = vec![false; positions.len()];
    for (tri, _) in triangles.iter().zip(selected.selected.iter()).filter(|(_, s)| **s) {
        for &v in tri {
            if let Some(slot) = on.get_mut(v as usize) {
                *slot = true;
            }
        }
    }
    let mut out = Vec::with_capacity(selected.count.saturating_mul(crate::scan_dots::DOT_SIZE));
    for (p, _) in positions.iter().zip(&on).filter(|(_, o)| **o) {
        cloud_dot(&mut out, &Dot { at: [p.x as f32, p.y as f32, p.z as f32], value: 1.0, rgb });
    }
    Some(out)
}
