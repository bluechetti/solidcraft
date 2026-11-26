---
name: solidcraft-scan-to-cad
description: Use when turning a scan (an STL, PLY or OBJ mesh measured off a real part, wall or object) into CAD in SolidCraft through its MCP server, or when a design holds a scan (a mesh body) to clean, align, fit, reconstruct, template or check a model against.
---

# SolidCraft scan to CAD

## Overview
A scan is a mesh body. Read it with `scan.survey {}`, clean and place it, choose a route by the kind of part, and check the result against the scan. `scan.survey {}` changes nothing and replies with `next`: the commands to run in order, each with `why` and `params`. Command shapes and routes are in `reference.md`, under "Work in detail" for each step below.

## Before the first call
1. Call `skill_reference {"skill":"solidcraft-scan-to-cad"}` (or read `reference.md`)., or one part: `{"skill":"solidcraft-scan-to-cad","section":"Known problems"}`. Modeling follows `solidcraft-modeling`.
2. `inspect_design {"measure":false,"sketches":false}`: scans are the mesh bodies. Ask before `new_design` or `open`.
3. No scan yet: `file.insert_mesh {"path": "…", "units": "m"}`. Give `units` if not millimetres.

## Work in this order
- **Ask once, when a person is there and the request does not say:** what the part is (`scan.plan {"process": …}`), whether it is a fitting replacement or a copy as scanned, and any callipered sizes (`scan.known_size`). A script or "just build it": none.
0. **First, the automatic route:** `scan.to_cad {}` cleans, reads and builds in one call and ends with a `verdict`. `met`: check it, done. Else work the open places in `verdict.places` and `next`, not from scratch., and accepts `{"tolerance": 0.2}`. Keep its body until your own measures better (`scan.deviation` on both).
1. **Read:** `scan.survey {}`. `next` is advice, not a script: read each `why`. Survey again after cleaning and `scan.find_all {}`.
2. **Units:** `units_hint` set: `mesh.units {"units":"m"}`.
3. **Clean, before fitting anything:** `scan.repair {}`, `scan.remove_floating {}`, `scan.denoise {}` (only if noise is a real share of the tolerance), `scan.fill_holes {}` (never on a wall or panel: its holes are real).
4. **Tolerance:** `scan.tolerance {"tolerance": 0.2}`: the user's accuracy, else about three times `noise`.
5. **Place:** `scan.stand_on_base {}`, `scan.quick_align {}`, or `scan.symmetry {}` then `scan.square_to_symmetry {}`. Several passes: `scan.align_all {}` or `scan.combine {}`.
6. **Route:** `scan.find_all {}`, then survey, by `shapes.in_shapes` and `sheet_like`: 0.6 or more `scan.reconstruct {}`, then `scan.reconstruct_rest {}`; 0.3 to 0.6 reconstruct, then `scan.fit_patches {}`; under 0.3 `scan.fit_patches {}` or `scan.free_form {}`; `sheet_like` `scan.flat_template {"outline":"scan"}`. Undo a step whose `fit.verdict` is `worse`. Reconstruct misreads: `scan.fit_plane {}`, `scan.fit_cylinder {}`, `scan.section {}`, then model.
7. **Verify** each stage with `scan.deviation {"body": "<model>"}` (`passes`, `why_not`, `max_above` missing, `max_below` extra), and `measure {}` against `own_size`.
8. **Model by hand or edit:** `reference.md`, "Scans and native features together".

## Red flags

| Thought | Do instead |
|---|---|
| "Build it step by step from the start" | `scan.to_cad {}` first; refine where its verdict says. |
| "Its model is poor, undo it" | Keep it until your own body measures better. |
| "Reconstruct proposed it, so it's right" | `scan.deviation {}` each stage. |
| "Fill the holes, it's cleaner" | Read `sheet_like` first. |
| "The scan is tiny" | `units_hint`: metres or inches. |
