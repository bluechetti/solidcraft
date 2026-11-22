//! The engine's integration tests, one binary for all of them (with `atomic_commands`, which has
//! its own). Each file under `tests/` is one module here, unchanged; a new file goes in this list
//! (sorted), or in Cargo.toml as its own `[[test]]` when it must run alone.

#[path = "mesh_export_regressions.rs"]
mod mesh_export_regressions;
#[path = "pleat_regressions.rs"]
mod pleat_regressions;
#[path = "save_as_cache.rs"]
mod save_as_cache;
#[path = "sheetmetal_documented_examples.rs"]
mod sheetmetal_documented_examples;
#[path = "shell_opening_regressions.rs"]
mod shell_opening_regressions;
#[path = "shell_regressions.rs"]
mod shell_regressions;
#[path = "sketch3d_profiles.rs"]
mod sketch3d_profiles;
#[path = "sketch_analysis.rs"]
mod sketch_analysis;
#[path = "sketch_drag_target.rs"]
mod sketch_drag_target;
#[path = "step_export_regressions.rs"]
mod step_export_regressions;
#[path = "step_merge_regressions.rs"]
mod step_merge_regressions;

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
