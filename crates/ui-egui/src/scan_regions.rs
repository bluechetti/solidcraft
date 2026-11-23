//! The regions a person marked on a scan (About this part in the agent chat, or `scan.plan_region`
//! with a role) in the viewport: each region's triangles (its kept selection) painted on the scan in
//! its role's colour, through the path the selection takes (`scan_selection`), and a tag at its
//! place naming it and what it is for: Focus here, Hold this size (with the size), Ignore, Don't
//! touch. Without it a region existed only as a chip, and "where is the part I said to leave
//! alone?" took a click to answer.
//!
//! Painted last of the scan's colourings: a map or a tool's marks the person opened wins where
//! they overlap (the first painting of a triangle is the one drawn); the tags stay.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use solidcraft_engine::doc::scan_plan::Role;
use solidcraft_viewport_gpu::view::{CloudStyle, ColorMode, GpuCloud};

use crate::SolveApp;
use crate::scan_dots::{Dot, cloud_dot};

/// The regions' id among the clouds the viewport draws.
const CLOUD_ID: u64 = u64::MAX - 8;

/// A dot's size on screen, in pixels.
const DOT_PIXELS: f32 = 3.0;

/// The most marked triangles painted; past it, their points are dots.
const PAINT_MOST: usize = 1_000_000;

thread_local! {
    /// The records last built: for which regions (a key of them and the scans), their version.
    static BUILT: RefCell<Option<(u64, u64, (Arc<Vec<u8>>, Arc<Vec<u8>>))>> = const { RefCell::new(None) };
    static VERSION: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// A role's colour, from the theme: focus the accent, hold amber, ignore dim, don't touch red.
pub(crate) fn role_colour(role: Role) -> egui::Color32 {
    let t = solidcraft_theme::Tokens::get();
    match role {
        Role::Focus => t.accent,
        Role::Hold => t.warning,
        Role::Ignore => t.text_dim,
        Role::Keep => t.error,
    }
}

/// A role's words, as the chat's buttons say them.
fn role_words(role: Role) -> &'static str {
    solidcraft_ui_text::tr(match role {
        Role::Focus => "ui.agent_chat_role_focus",
        Role::Hold => "ui.agent_chat_role_hold",
        Role::Ignore => "ui.agent_chat_role_ignore",
        Role::Keep => "ui.agent_chat_role_keep",
    })
}

/// One marked region as drawn: its scan, name, role, held size, place and triangles (one flag a
/// triangle of the scan as it is shown).
struct Marked {
    scan: String,
    name: String,
    role: Role,
    hold: Option<f64>,
    at: [f64; 3],
    flags: Vec<bool>,
}

/// The regions with a role and an extent on scans shown now, their kept selections still of the
/// scan as it is (a scan made again with other triangles leaves them out).
fn marked(app: &SolveApp) -> Vec<Marked> {
    let session = &app.session;
    let mut out = Vec::new();
    for (scan, plan) in &session.doc.scan_plans {
        if !session.body_visible(scan) {
            continue;
        }
        let Some(mesh) = solidcraft_engine::cmd::scan::shown_mesh(session, scan) else { continue };
        for r in &plan.regions {
            let (Some(role), Some(name)) = (r.role, r.selection.as_deref()) else { continue };
            let Some(kept) = session.doc.scan_selections.iter().find(|k| k.name == name && k.body == *scan) else { continue };
            if kept.triangles as usize != mesh.triangles.len() {
                continue;
            }
            out.push(Marked { scan: scan.clone(), name: r.name.clone(), role, hold: r.hold, at: r.at, flags: kept.flags() });
        }
    }
    out
}

/// What the painting depends on: the regions, their selections and colours, and each scan's
/// triangles where they are held (a moved or remeshed scan is painted again).
fn key(app: &SolveApp) -> u64 {
    let session = &app.session;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (scan, plan) in &session.doc.scan_plans {
        scan.hash(&mut h);
        session.body_visible(scan).hash(&mut h);
        if let Some(mesh) = solidcraft_engine::cmd::scan::shown_mesh(session, scan) {
            (Arc::as_ptr(&mesh) as usize, mesh.triangles.len()).hash(&mut h);
        }
        for r in &plan.regions {
            (&r.name, r.role.map(|x| x as u8), &r.selection).hash(&mut h);
            if let (Some(role), Some(name)) = (r.role, r.selection.as_deref()) {
                role_colour(role).to_array().hash(&mut h);
                if let Some(kept) = session.doc.scan_selections.iter().find(|k| k.name == name && k.body == *scan) {
                    (kept.triangles, kept.runs.len(), kept.count()).hash(&mut h);
                }
            }
        }
    }
    h.finish()
}

/// The marked regions as a cloud for the GPU viewport, or nothing when no scan has one.
pub(crate) fn gpu_cloud(app: &SolveApp) -> Option<GpuCloud> {
    if app.session.doc.scan_plans.values().all(|p| p.regions.iter().all(|r| r.role.is_none() || r.selection.is_none())) {
        return None;
    }
    let key = key(app);
    let (version, (dots, triangles)) = BUILT.with(|b| {
        let mut b = b.borrow_mut();
        if let Some((k, version, records)) = b.as_ref()
            && *k == key
        {
            return Some((*version, records.clone()));
        }
        let records = records(app, &marked(app))?;
        let version = VERSION.with(|v| {
            v.set(v.get() + 1);
            v.get()
        });
        *b = Some((key, version, records.clone()));
        Some((version, records))
    })?;
    Some(GpuCloud {
        id: CLOUD_ID,
        version,
        dots,
        segments: crate::scan_dots::none(),
        triangles,
        style: CloudStyle { mode: ColorMode::Own, range: [0.0, 1.0], filters: Default::default(), point_size: DOT_PIXELS },
    })
}

/// Each region's triangles painted in its role's colour (dots: none), or past [`PAINT_MOST`]
/// triangles one dot for each point of them (triangles: none). `None` when nothing is marked.
fn records(app: &SolveApp, marked: &[Marked]) -> Option<(Arc<Vec<u8>>, Arc<Vec<u8>>)> {
    if marked.is_empty() {
        return None;
    }
    let total: usize = marked.iter().map(|m| m.flags.iter().filter(|f| **f).count()).sum();
    let mut painted = Vec::new();
    let mut dots = Vec::new();
    for m in marked {
        let Some(shown) = solidcraft_engine::cmd::scan::shown_mesh(&app.session, &m.scan) else { continue };
        let (positions, triangles) = (shown.positions.as_slice(), shown.triangles.as_slice());
        let rgb = {
            let c = role_colour(m.role);
            [c.r(), c.g(), c.b()]
        };
        if total <= PAINT_MOST {
            let lifts = crate::scan_dots::PaintLifts::of(positions, triangles);
            for (tri, _) in triangles.iter().zip(&m.flags).filter(|(_, on)| **on) {
                lifts.paint(&mut painted, positions, *tri, 1.0, rgb);
            }
        } else {
            let mut on = vec![false; positions.len()];
            for (tri, _) in triangles.iter().zip(&m.flags).filter(|(_, on)| **on) {
                for &v in tri {
                    if let Some(slot) = on.get_mut(v as usize) {
                        *slot = true;
                    }
                }
            }
            for (p, _) in positions.iter().zip(&on).filter(|(_, o)| **o) {
                cloud_dot(&mut dots, &Dot { at: [p.x as f32, p.y as f32, p.z as f32], value: 1.0, rgb });
            }
        }
    }
    Some((Arc::new(dots), Arc::new(painted)))
}

/// A tag at each marked region: its name and what it is for (and a held size), in its colour.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    use egui::{FontId, Stroke, vec2};
    let marked = marked(app);
    if marked.is_empty() {
        return;
    }
    let t = solidcraft_theme::Tokens::get();
    let font = FontId::proportional(t.font_small);
    let mut tags = crate::view_tags::Tags::default();
    for m in &marked {
        let Some(at) = proj.to_screen(solidcore_record::Vec3::new(m.at[0], m.at[1], m.at[2])) else { continue };
        let colour = role_colour(m.role);
        let size = m.hold.map_or(String::new(), |h| format!(" {}", crate::prefs::show_mm(app, h, 1)));
        let galley = painter.layout_no_wrap(format!("{} · {}{size}", m.name, role_words(m.role)), font.clone(), t.text);
        let rect = tags.place(at, galley.size() + vec2(10.0, 6.0));
        painter.line_segment([at, rect.left_bottom()], Stroke::new(1.0, colour));
        painter.circle_filled(at, 3.5, colour);
        painter.rect_filled(rect, 3.0, t.panel);
        painter.rect_stroke(rect, 3.0, Stroke::new(1.5, colour), egui::StrokeKind::Inside);
        painter.galley(rect.min + vec2(5.0, 3.0), galley, t.text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A region kept with a role is painted on its scan and tagged; one with no extent, or a scan
    /// with none, paints nothing.
    #[test]
    fn a_marked_region_is_painted_in_its_roles_colour() {
        let mut h = crate::scenario::Harness::new();
        assert!(gpu_cloud(&h.app).is_none(), "nothing marked");
        let path = std::env::temp_dir().join(format!("solidcraft-regions-{}.stl", std::process::id()));
        let mut stl = String::from("solid p\n");
        for i in 0..10 {
            for j in 0..10 {
                let (x, y) = (2.0 * f64::from(i), 2.0 * f64::from(j));
                for tri in [[(x, y), (x + 2.0, y), (x + 2.0, y + 2.0)], [(x, y), (x + 2.0, y + 2.0), (x, y + 2.0)]] {
                    stl += "facet normal 0 0 1\nouter loop\n";
                    for (a, b) in tri {
                        stl += &format!("vertex {a} {b} 0\n");
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        stl += "endsolid p\n";
        std::fs::write(&path, stl).unwrap();
        let scan = h.app.run("file.insert_mesh", json!({"path": path})).unwrap()["name"].as_str().unwrap().to_owned();
        let _ = std::fs::remove_file(&path);
        h.app.run("scan.plan_region", json!({"scan": scan, "name": "Spot", "at": [1, 1, 0], "role": "focus"})).unwrap();
        assert!(gpu_cloud(&h.app).is_none(), "a region with no extent paints nothing");
        h.app.run("scan.select", json!({"brush": {"points": [[10, 10, 0]], "radius": 3}})).unwrap();
        h.app.run("scan.plan_region", json!({"scan": scan, "name": "Lip", "selected": true, "role": "keep"})).unwrap();
        let cloud = gpu_cloud(&h.app).expect("painted");
        assert!(!cloud.triangles.is_empty() && cloud.dots.is_empty(), "painted on the triangles");
        let marked = marked(&h.app);
        assert_eq!(marked.iter().map(|m| (m.name.as_str(), m.role)).collect::<Vec<_>>(), [("Lip", Role::Keep)]);
        // The same regions: the same painting (no rebuild every frame).
        assert_eq!(gpu_cloud(&h.app).map(|c| c.version), Some(cloud.version));
        h.app.run("scan.plan_region", json!({"scan": scan, "name": "Lip", "role": "ignore"})).unwrap();
        assert!(gpu_cloud(&h.app).is_some_and(|c| c.version != cloud.version), "a new role is painted again");
    }
}
