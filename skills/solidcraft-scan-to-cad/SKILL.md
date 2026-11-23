---
name: solidcraft-scan-to-cad
description: Use when turning a scan (an STL, PLY or OBJ mesh measured off a real part, wall or object) into CAD in SolidCraft through its MCP server, or when a design holds a scan (a mesh body) to clean, align, fit, reconstruct, template or check a model against.
---

# SolidCraft scan to CAD

## Overview
A scan is a mesh body. Read it with `scan.survey {}`, clean and place it, choose a route by the kind of part, and check the result against the scan. `scan.survey {}` changes nothing and replies with `next`: the commands to run in order, each with `why` and `params`. Command shapes, routes and known problems are in `reference.md`.

## Before the first call
1. Call `skill_reference {"skill":"solidcraft-scan-to-cad"}` (or read `reference.md`). Later, one part of it again: `{"skill":"solidcraft-scan-to-cad","section":"Known problems"}` (`"section":"contents"` lists them). Modeling after the scan is read follows `solidcraft-modeling`.
2. `inspect_design {"measure":false,"sketches":false}`: the scans are the mesh bodies. Ask before `new_design` or `open`.
3. No scan yet: `file.insert_mesh {"path": "…", "units": "m"}`. Give `units` when the file is not in millimetres.

## Work in this order
1. **Read:** `scan.survey {}`. Read every `why` before running its step: `next` is advice, not a script. Survey again after cleaning and after `scan.find_all {}`.
2. **Units:** `units_hint` set → `mesh.units {"units":"m"}`.
3. **Clean, before fitting anything** (fits are not redone after a clean): `scan.repair {}`, `scan.remove_floating {}`, `scan.denoise {}` (only when the noise is a real share of the tolerance), `scan.fill_holes {}` (never on a wall or panel: its holes are real).
4. **Tolerance:** `scan.tolerance {"tolerance": 0.2}`: the user's accuracy, else about three times `noise`. Every check reads against it.
5. **Place:** `scan.stand_on_base {}`, `scan.quick_align {}`, or `scan.symmetry {}` then `scan.square_to_symmetry {}`. Several passes: `scan.align_all {}` or `scan.combine {}`.
6. **Route:** `scan.find_all {}`, then survey: by `shapes.in_shapes` and `sheet_like`:
   - 0.6 or more: `scan.reconstruct {}`, then run its `proposals` one by one; undo a step whose `fit.verdict` is `worse`.
     Then what is still missing: `scan.reconstruct_rest {}` (each piece the body leaves unexplained read and built, worse steps taken back, one undo step), or by hand: `scan.select {"unexplained": "<scan> Reconstructed"}`, `scan.reconstruct_selection {}`, run those proposals; repeat until `pieces` is empty or a piece adds nothing (then `"piece": 1`).
   - 0.3 to 0.6: reconstruct, then `scan.fit_patches {}` over the freeform rest.
   - under 0.3: `scan.fit_patches {}` or `scan.free_form {}`.
   - `sheet_like`: `scan.flat_template {"outline":"scan"}`.
   - Reconstruct misreads: fit by hand with `scan.fit_plane {}` and `scan.fit_cylinder {}`, read sizes off cuts with `scan.section {"plane":"XY","offset":12}` (outlines as data, nothing made, no undo), then model. `scan.section_sketch {}` only for a cut you will build from.
7. **Verify** each stage with `scan.deviation {"body": "<model>"}`: `passes` (both ways; `why_not` says what failed), `max_above` (material missing), `max_below` (extra), `model_farthest_at` (made up). Compare `measure {}` with `own_size`.
8. **Model by hand or edit:** `reference.md`, "Scans and native features together".

## Red flags

| Thought | Do instead |
|---|---|
| "List the scan commands" | `scan.survey {}`, then `reference.md`. |
| "Reconstruct proposed it, so it's right" | `scan.deviation {}` after each stage. |
| "Fill the holes, it's cleaner" | Read `sheet_like` first. |
| "The scan is tiny" | `units_hint`: saved in metres or inches. |
| "Guess a point on the scan" | Use a `point` from `scan.find_all {}` shapes. |
| "Section sketch, inspect it, undo" to read a size | `scan.section {}`: the cut as data in one call. |
| "`measure` matches" | `scan.deviation {}` too. |
