//! The UI's integration tests, one binary for all of them (with `scenarios` and `icon_audit`,
//! which have their own: CI, the quarantine list and the icon workflow name them). Each file
//! under `tests/` is one module here, unchanged; a new file goes in this list (sorted), or in
//! Cargo.toml as its own `[[test]]` when it must run alone. A test's name gains its file as a
//! prefix: `--test integration <file>::` runs what `--test <file>` ran.

// The files keep their own `mod support;` / `#[path = "support/raster.rs"]`, unchanged, so each
// has its copy of those helpers as when it was its own binary (as solidcore's merged tests do).
#![allow(clippy::duplicate_mod)]

#[path = "assistant_render.rs"]
mod assistant_render;
#[path = "broken_view_preview.rs"]
mod broken_view_preview;
#[path = "browser_search.rs"]
mod browser_search;
#[path = "coincident_circles.rs"]
mod coincident_circles;
#[path = "coincident_lines.rs"]
mod coincident_lines;
#[path = "constraint_preselection.rs"]
mod constraint_preselection;
#[path = "dialog_chip_hover.rs"]
mod dialog_chip_hover;
#[path = "disclosure_render.rs"]
mod disclosure_render;
#[path = "joint_move_render.rs"]
mod joint_move_render;
#[path = "orbit_empty_space_pivot.rs"]
mod orbit_empty_space_pivot;
#[path = "parameter_enter.rs"]
mod parameter_enter;
#[path = "parameter_order.rs"]
mod parameter_order;
#[path = "plugin_resource_preferences.rs"]
mod plugin_resource_preferences;
#[path = "point_alignment.rs"]
mod point_alignment;
#[path = "point_constraint_preselection.rs"]
mod point_constraint_preselection;
#[path = "popup_owner_capture.rs"]
mod popup_owner_capture;
#[path = "popup_regressions.rs"]
mod popup_regressions;
#[path = "projection_preselection.rs"]
mod projection_preselection;
#[path = "stand_up_offer.rs"]
mod stand_up_offer;
#[path = "ui_layout.rs"]
mod ui_layout;
#[path = "window_widths.rs"]
mod window_widths;
#[path = "woken_centre.rs"]
mod woken_centre;

/// With `autotests = false`, a file under `tests/` that nothing names is never built or run: every
/// one is a module above or a `[[test]]` of its own in Cargo.toml.
#[test]
fn every_test_file_is_built() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let here = include_str!("integration.rs");
    let manifest = include_str!("../Cargo.toml");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(root.join("tests")).expect("tests/ is readable") {
        let name = entry.expect("a directory entry").file_name().to_string_lossy().into_owned();
        if !name.ends_with(".rs") || name == "integration.rs" {
            continue;
        }
        if !here.contains(&format!("#[path = \"{name}\"]")) && !manifest.contains(&format!("path = \"tests/{name}\"")) {
            missing.push(name);
        }
    }
    assert!(missing.is_empty(), "test files no binary builds (add each to tests/integration.rs): {missing:?}");
}
