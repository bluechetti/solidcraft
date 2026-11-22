# Scan cases

Parts with exact answers for the scan-to-CAD benchmark (`cargo xtask scan-bench`).

Each case is a folder:
- `recipe.json`: the part, built by ordinary modelling commands. Every size is the recipe's, so
  the truth is exact and nothing is measured off another program.
- `expected.json`: what a rebuilt model is held to: the volume in closed form where there is a
  short one (`volume_by` shows it), the part's cylinders (holes and bosses: radius, a point on
  the axis, its direction) and how many holes go the whole way round.

The benchmark meshes the part, degrades the mesh with a fixed seed (noise, missing areas, pose,
scale: `crates/testkit/src/scan_degrade.rs`), rebuilds it through the scan commands a person
would run, and scores the result against these files and the bars in `tests/scan-bench.toml`.
