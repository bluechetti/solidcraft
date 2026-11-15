//! Section Analysis with a scan in the design: where the section plane cuts each shown scan is
//! drawn over the picture in the mesh colour, beside the model's own cut, so the two cross
//! sections can be compared at a glance as the plane moves. Off with the preference
//! `section_scan_outline`.

use std::cell::RefCell;
use std::sync::Arc;

use solidcore_record::Vec3;

use crate::SolveApp;

/// The cuts: each a run of points in the world and whether it closes on itself.
type Cuts = Arc<Vec<(Vec<Vec3>, bool)>>;

thread_local! {
    /// The cuts last read, for which design revision, plane and hidden bodies.
    static CUT: RefCell<Option<(Key, Cuts)>> = const { RefCell::new(None) };
}

#[derive(Clone, PartialEq)]
struct Key {
    revision: u64,
    plane: [u64; 6],
    hidden: Vec<String>,
}

/// The cuts through the shown scans for the section shown now, read again only when the design,
/// the plane or what is hidden changes; `None` with no section or with the preference off.
fn cuts(app: &SolveApp) -> Option<Cuts> {
    if !app.preferences.section_scan_outline {
        return None;
    }
    let (o, n) = app.session.section?;
    let key = Key {
        revision: app.session.revision,
        plane: [o.x, o.y, o.z, n.x, n.y, n.z].map(f64::to_bits),
        hidden: app.session.visibility.hidden_bodies.clone(),
    };
    CUT.with(|c| {
        let mut c = c.borrow_mut();
        if let Some((k, cuts)) = c.as_ref()
            && *k == key
        {
            return Some(cuts.clone());
        }
        let cuts: Cuts = Arc::new(solidcraft_engine::cmd::section::scan_outlines(&app.session));
        *c = Some((key, cuts.clone()));
        Some(cuts)
    })
}

/// The scans' cuts over the picture while Section Analysis is on.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(cuts) = cuts(app) else { return };
    let t = solidcraft_theme::Tokens::get();
    let stroke = egui::Stroke::new(2.0, t.mesh_accent);
    for (run, closed) in cuts.iter() {
        let mut line: Vec<egui::Pos2> = run.iter().filter_map(|p| proj.to_screen(*p)).collect();
        // A run partly behind the eye is left out rather than drawn with a jump.
        if line.len() != run.len() || line.len() < 2 {
            continue;
        }
        if *closed && let Some(&first) = line.first() {
            line.push(first);
        }
        painter.add(egui::Shape::line(line, stroke));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A scan of a 10 mm cube sectioned at half height: one closed square run round it, drawn
    /// until the section is cleared or the preference is off.
    #[test]
    fn a_section_cuts_the_scan_and_the_preference_turns_it_off() {
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
        assert!(cuts(&h.app).is_none(), "no section, no cut");

        let reply = h.app.run("inspect.section", json!({"plane": "XY", "at": 5})).unwrap();
        assert_eq!(reply["scan_runs"], 1, "{reply}");
        let found = cuts(&h.app).expect("the cut");
        assert_eq!(found.len(), 1, "one run round the cube");
        let (run, closed) = &found[0];
        assert!(*closed && run.iter().all(|p| (p.z - 5.0).abs() < 1e-9), "closed, on the plane: {run:?}");
        assert!(
            run.iter().all(|p| p.x.abs() < 1e-9 || (p.x - 10.0).abs() < 1e-9 || p.y.abs() < 1e-9 || (p.y - 10.0).abs() < 1e-9),
            "round the square"
        );

        h.app.preferences.section_scan_outline = false;
        assert!(cuts(&h.app).is_none(), "off: as before");
        h.app.preferences.section_scan_outline = true;
        h.app.run("inspect.section", json!({"clear": true})).unwrap();
        assert!(cuts(&h.app).is_none(), "cleared");
    }
}
