//! The Scan workspace: scan to CAD (#8615). It works on the same design, history and selection
//! as Design; only the ribbon tabs change, to SCAN (its tools in working order) and the shared
//! VIEW. A sketch started here opens the SKETCH tab and finishing it comes back to SCAN.

use crate::SolveApp;

/// The ribbon tabs of the Scan workspace, in row order.
pub const TABS: &[&str] = &["SCAN", "VIEW"];

/// Where the workspace opens, and where it goes back to when its tab is not shown.
pub const HOME: &str = "SCAN";

pub fn active(app: &SolveApp) -> bool {
    app.ui.scan_workspace && !crate::render_workspace::active(app)
}

/// Open the Scan workspace (`true`) or go back to Design (`false`). Like Render, it waits for
/// a sketch, tool or dialog to finish, so the ribbon never changes under an edit.
pub fn enter(app: &mut SolveApp, scan: bool) -> Result<(), String> {
    if scan == active(app) {
        return Ok(());
    }
    if app.session.active_sketch.is_some() || app.tool.is_some() || app.dialog.is_some() || app.file_load.busy() {
        return Err(solidcraft_ui_text::tr("ui.workspace_finish_operation").into());
    }
    if crate::render_workspace::active(app) {
        crate::render_workspace::enter(app, false)?;
    }
    app.ui.scan_workspace = scan;
    app.ui.sketch_return_tab = None;
    app.ui.tab = if scan { HOME } else { "MODEL" }.into();
    crate::workspace::sync_context(app);
    Ok(())
}

/// Go to a workspace by name: design, scan or render.
pub fn switch(app: &mut SolveApp, workspace: &str) -> Result<(), String> {
    match workspace {
        "scan" => enter(app, true),
        "render" => {
            crate::render_workspace::enter(app, true)?;
            // Leaving Render goes back to Design.
            app.ui.scan_workspace = false;
            Ok(())
        }
        _ if crate::render_workspace::active(app) => crate::render_workspace::enter(app, false),
        _ => enter(app, false),
    }
}

/// The open workspace's name, as `switch` takes it.
pub fn current(app: &SolveApp) -> &'static str {
    if crate::render_workspace::active(app) {
        "render"
    } else if active(app) {
        "scan"
    } else {
        "design"
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn the_scan_workspace_swaps_the_ribbon_tabs_and_keeps_the_design() {
        let mut h = crate::scenario::Harness::new();
        h.app.run("solid.box", json!({"body_name":"Block", "operation":"new", "length":10, "width":10, "height":10})).unwrap();
        super::enter(&mut h.app, true).unwrap();
        assert!(super::active(&h.app));
        assert_eq!(h.app.ui.tab, "SCAN");
        assert_eq!(crate::workspace::visible_tabs(&h.app), ["SCAN", "VIEW"]);
        assert!(h.app.session.model.state().bodies.iter().any(|b| b.name == "Block"), "the same design");

        // A sketch opens its tab and finishing it comes back to SCAN, inside the workspace.
        h.app.run("sketch.create", json!({"plane":"XY"})).unwrap();
        assert_eq!(h.app.ui.tab, "SKETCH");
        assert_eq!(crate::workspace::visible_tabs(&h.app), ["SCAN", "VIEW", "SKETCH"]);
        assert!(super::enter(&mut h.app, false).is_err(), "the switcher waits for the sketch");
        h.app.run("sketch.rectangle.two_point", json!({"p0":[0,0],"p1":[4,3]})).unwrap();
        h.app.run("sketch.finish", json!({})).unwrap();
        assert_eq!(h.app.ui.tab, "SCAN");

        super::enter(&mut h.app, false).unwrap();
        assert!(!super::active(&h.app));
        assert_eq!(h.app.ui.tab, "MODEL");
        assert!(!crate::workspace::visible_tabs(&h.app).contains(&"SCAN"), "SCAN is not a design tab");
    }

    /// Every tool of the Scan tab is a button there; the ones not built yet are greyed with the
    /// issue that tracks them, and refuse when called.
    #[test]
    fn the_scan_tab_shows_its_layout_with_planned_tools_greyed() {
        let specs = solidcraft_engine::command_specs();
        assert_eq!(crate::workspace::panels("SCAN"), ["INSERT", "CLEAN", "ALIGN", "RECONSTRUCT", "FREEFORM", "SKETCH", "CHECK"]);
        let mut h = crate::scenario::Harness::new();
        super::enter(&mut h.app, true).unwrap();
        h.frames(2);
        // The tools most work needs are buttons, in working order; the rest are in the drop-downs.
        for id in [
            "file.insert_mesh",
            "scan.survey",
            "scan.clean_up",
            "scan.select",
            "scan.remove_floating",
            "scan.fill_holes",
            "scan.stand_on_base",
            "scan.align",
            "scan.reconstruct",
            "scan.find_all",
            "scan.fit_plane",
            "scan.free_form",
            "scan.wrap_surface",
            "scan.fit_patches",
            "scan.edit_surface",
            "scan.section_sketch",
            "scan.curve_on_scan",
            "scan.flat_template",
            "scan.deviation",
            "scan.align_to_model",
        ] {
            assert!(crate::scenario::handle_at(&format!("button:{id}")).is_some(), "{id} is a button on the Scan tab");
        }
        // The placing tools are built; the rest are greyed until they are.
        let built = [
            "scan.bases",
            "scan.stand_on_base",
            "scan.rest_on_grid",
            "scan.center",
            "scan.quick_align",
            "scan.symmetry",
            "scan.square_to_symmetry",
            "scan.fit_plane",
            "scan.fit_cylinder",
            "scan.deviation",
            "scan.fit_sphere",
            "scan.fit_surface",
            "scan.complete_by_mirroring",
            "scan.edit_surface",
            "scan.curve_on_scan",
            "scan.remove_floating",
            "scan.fill_holes",
            "scan.reduce",
            "scan.repair",
            "scan.section_sketch",
            "scan.free_form",
            "scan.edit_net",
            "scan.wrap_surface",
            "scan.merge_surfaces",
            "scan.slice_surface",
            "scan.close_surface",
            "scan.make_flat",
            "scan.fit_cone",
            "scan.fit_torus",
            "scan.find_all",
            "scan.reconstruct",
            "scan.reconstruct_selection",
            "scan.reconstruct_rest",
            "scan.extrude_it",
            "scan.revolve_it",
            "scan.loft_it",
            "scan.pipe_it",
            "scan.coil_it",
            "scan.shell_it",
            "scan.cut_it",
            "scan.hole_it",
            "scan.pattern_it",
            "scan.fillet_it",
            "scan.chamfer_it",
            "scan.register",
            "scan.mesh_from_points",
            "scan.remesh",
            "scan.denoise",
            "scan.sections_along_curve",
            "scan.select",
            "scan.select_inside_curve",
            "scan.keep_selection",
            "scan.delete_selection",
            "scan.curve_from_selection",
            "scan.split",
            "scan.align_all",
            "scan.combine",
            "scan.align_pairs",
            "scan.align_to_model",
            "scan.fillet",
            "scan.flatten",
            "scan.flat_template",
            "scan.panel_export",
            "scan.panel_proposals",
            "scan.roll_onto_scan",
            "scan.fit_patches",
            "scan.add_face",
            "scan.mirror_free_form",
            "scan.refit",
            "scan.tolerance",
            "scan.survey",
            "scan.clean_up",
            "scan.align",
        ];
        for c in specs.iter().filter(|c| c.id.starts_with("scan.")) {
            if built.contains(&c.id) {
                assert!(c.planned.is_none(), "{}", c.id);
                continue;
            }
            assert_eq!(c.planned, Some("8615"), "{}", c.id);
            assert!(h.app.run(c.id, json!({})).is_err(), "{} refuses until it is built", c.id);
        }
    }
}
