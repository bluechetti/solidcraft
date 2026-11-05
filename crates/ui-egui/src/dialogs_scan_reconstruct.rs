//! Reconstruct (`scan.reconstruct`): what Find All's shapes make, and the steps that build it as
//! ordinary features. The button reads the scan first (Find All runs when it has not, and the scan
//! is coloured by its regions); the dialog then lists each step Reconstruct proposes (Revolve It
//! for a turned part, Extrude It for each prism, Cut It for each round hole and Pattern It for a
//! row or ring of them), all ticked, and OK builds the ticked ones in order, each its own undo
//! step.

use serde_json::Value;
use solidcraft_engine::cmd::scan::reconstruct::{self, Proposal, ScanShapes};

use crate::SolveApp;
use crate::dialogs::Kind;
use crate::ui_kit;

/// The Reconstruct dialog's values: which steps are ticked, by their place in the proposals.
#[derive(Clone, Debug, Default)]
pub struct Reconstruct {
    pub ticked: Vec<bool>,
}

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<crate::selection::SelInput>)> {
    (id == "scan.reconstruct").then(|| (Kind::ScanReconstruct(Reconstruct::default()), Vec::new()))
}

/// Read the scan before the dialog opens: false (and nothing opens) when it cannot be read.
pub fn read_first(app: &mut SolveApp) -> bool {
    app.run("scan.reconstruct", serde_json::json!({})).is_ok()
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

fn found(app: &SolveApp) -> Option<std::sync::Arc<ScanShapes>> {
    app.session.scan_shapes.clone().filter(|k| k.current(&app.session))
}

/// A step's line in the list.
fn label(app: &SolveApp, found: &ScanShapes, step: &Proposal) -> String {
    let reading = &found.reading;
    let mm = |v: f64| crate::prefs::show_mm(app, v, 1);
    match *step {
        Proposal::Revolve => {
            let faces = reading.revolve.as_ref().map_or(0, |r| r.members.len());
            crate::i18n::format("ui.scan_reconstruct_revolve", &[("faces", &faces.to_string())])
        }
        Proposal::Extrude(k) => {
            let (depth, walls) = reading.prisms.get(k).map_or((0.0, 0), |p| (p.depth(), p.walls.len()));
            crate::i18n::format(
                "ui.scan_reconstruct_extrude",
                &[("number", &(k + 1).to_string()), ("depth", &mm(depth)), ("walls", &walls.to_string())],
            )
        }
        Proposal::Cut(k) => {
            let (diameter, depth) = reading.holes.get(k).map_or((0.0, 0.0), |h| (2.0 * h.radius, h.depth()));
            crate::i18n::format("ui.scan_reconstruct_cut", &[("number", &(k + 1).to_string()), ("diameter", &mm(diameter)), ("depth", &mm(depth))])
        }
        Proposal::Pattern(k) => {
            let count = reading.holes.get(k).and_then(|h| h.repeat).map_or(0, |r| r.0);
            crate::i18n::format("ui.scan_reconstruct_pattern", &[("number", &(k + 1).to_string()), ("count", &count.to_string())])
        }
    }
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut Reconstruct) -> bool {
    let Some(found) = found(app) else {
        ui_kit::text(ui, tr("ui.scan_reconstruct_unread"), ui_kit::Text::Report("scan_reconstruct_unread", true));
        return false;
    };
    let steps = reconstruct::proposals(&found.reading);
    k.ticked.resize(steps.len(), true);
    let summary = crate::i18n::format(
        "ui.scan_reconstruct_found",
        &[("shapes", &found.reading.faces.len().to_string()), ("share", &format!("{:.0} %", 100.0 * found.outcome.in_shapes))],
    );
    ui_kit::text(ui, &summary, ui_kit::Text::Note("scan_reconstruct_found"));
    if steps.is_empty() {
        ui_kit::text(ui, tr("ui.scan_reconstruct_nothing"), ui_kit::Text::Report("scan_reconstruct_nothing", true));
    }
    for (step, ticked) in steps.iter().zip(k.ticked.iter_mut()) {
        ui_kit::checkbox(ui, &label(app, &found, step), ticked);
    }
    ui_kit::text(ui, tr("ui.scan_reconstruct_hint"), ui_kit::Text::Hint("scan_reconstruct_hint"));
    false
}

/// What OK runs: the ticked steps, in order.
pub fn commands(app: &SolveApp, k: &Reconstruct) -> Result<Vec<(String, Value)>, String> {
    let found = found(app).ok_or_else(|| tr("ui.scan_reconstruct_unread").to_string())?;
    let chosen: Vec<(String, Value)> = reconstruct::proposals(&found.reading)
        .iter()
        .zip(k.ticked.iter().copied().chain(std::iter::repeat(true)))
        .filter(|(_, ticked)| *ticked)
        .map(|(step, _)| (step.command().to_string(), step.params(&found.scan)))
        .collect();
    if chosen.is_empty() {
        return Err(tr("ui.scan_reconstruct_none_ticked").into());
    }
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A rounded slab as a scan: Reconstruct's button reads it and lists one Extrude It, ticked;
    /// OK builds the slab.
    #[test]
    fn reconstruct_lists_the_prism_and_ok_extrudes_it() {
        let mut h = crate::scenario::Harness::new();
        // A slab 30 × 20 × 6, each face a fine grid, as a scan.
        let (n, size) = (10u32, [30.0, 20.0, 6.0]);
        let mut stl = String::from("solid s\n");
        for (axis, high) in [(0, false), (0, true), (1, false), (1, true), (2, false), (2, true)] {
            let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
            for j in 0..n {
                for i in 0..n {
                    let at = |di: u32, dj: u32| {
                        let mut p = [0.0; 3];
                        p[axis] = if high { size[axis] } else { 0.0 };
                        p[u] = size[u] * f64::from(i + di) / f64::from(n);
                        p[v] = size[v] * f64::from(j + dj) / f64::from(n);
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
        let path = h.dir.join("slab.stl");
        std::fs::write(&path, stl + "endsolid s\n").unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        h.app.start("scan.reconstruct");
        assert!(matches!(h.app.dialog.as_ref().map(|d| &d.kind), Some(Kind::ScanReconstruct(_))), "the dialog is open");
        let found = found(&h.app).expect("read");
        let steps = reconstruct::proposals(&found.reading);
        assert!(matches!(steps.first(), Some(Proposal::Extrude(0))), "{steps:?}");
        let commands = commands(&h.app, &Reconstruct { ticked: vec![true; steps.len()] }).unwrap();
        assert_eq!(commands[0].0, "scan.extrude_it");
        for (id, p) in commands {
            h.app.run(&id, p).unwrap();
        }
        let built = h.app.session.model.state().bodies.iter().find(|b| b.name.ends_with("Reconstructed")).and_then(|b| b.body.volume_at(0.01));
        assert!(built.is_some_and(|v| (v - 3600.0).abs() < 1.0), "{built:?}");
    }
}
