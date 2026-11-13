# Scan to CAD reference

Command shapes and replies for the scan workflow in `SKILL.md`. Every command takes `body` (the scan) and defaults to the selected or only mesh body. Run a command with `execute {"command":"scan.survey","params":{}}` or as a `batch` step. `describe_command {"id": "scan.fit_patches"}` gives one command's full parameters and schemas.

## What the survey says
`scan.survey {}` reads the scan in about a second per 2 million triangles and changes nothing. Its reply:
- **Size:** `own_size` (along its own directions, longest first), `low`, `high`, `center`, `area`, `typical_edge`. `units_hint` appears when the scan is under 3 mm across.
- **Noise:** `noise` (mm), read at a few places, and `smoothing`, the Denoise strength it would take (0: leave it).
- **Faults:** `holes` and `longest_hole` (holes inside the rims), `rims` (a scan seen from one side ends at one), `tangled_edges` (open edges no hole closes: run Repair), `pieces` and `floating` (scraps far smaller than the part).
- **Kind:** `flat_share` (area in broad flat faces), `sheet_like` (a wall or a panel at least a metre long), and `shapes` (area shares by kind, once Find All has run on the scan as it is now).
- **Context:** `tolerance` and `tolerance_set`, `other_scans`, `models` (solid and surface bodies to check against).
- **`next`:** `[{command, why, params}]` in the order the work goes: units, clean, align, denoise, holes, tolerance, place, route, check. Each `why` names the number that triggered it.

## Routes in detail
- **Every build step** (Extrude It, Revolve It, Cut It, Pattern It, Fillet It, Chamfer It) replies with `fit`: the body measured against a sample of the scan before and after, `explained` (share of the scan within the limit of the body), `within` (share within the tolerance), `rms`, and `verdict` first, better, same or worse. Undo a step that comes back `worse` with `edit.undo {}` and try the next proposal or a region.
- **Machined or turned part:** `scan.reconstruct {}` replies with `prisms`, `turned`, `holes`, `repeats` and `proposals`, the build steps in order. Run them in that order: Extrude It or Revolve It first, then Cut It, Pattern It, Fillet It and Chamfer It, all into the body `<scan> Reconstructed`. Put each risky step (the first boolean onto a revolve, a fillet) in a batch of its own. For a part too busy to read whole, select one region with `scan.select {"shape": 3}`, a brush or a polygon, then run `scan.reconstruct_selection {}`.
- **What the body still lacks:** after the proposals, `scan.select {"unexplained": "<scan> Reconstructed"}` selects the largest piece of the scan the body does not explain (farther from it than five times the tolerance, at least 0.5 mm) and lists every such piece in `pieces` (`area`, `side`: `missing` material the body lacks or `extra` material it should not have, such as a hole not cut, `deepest`, `point`). Run `scan.reconstruct_selection {}` on it and its proposals; on a piece that adds nothing, take the next with `{"unexplained": {"body": "<scan> Reconstructed", "piece": 1}}`; `"grow": 2` takes a little of the faces round it. Stop when `pieces` is empty. On the shape-family benchmark this took a block machined from several sides from 14% to 57% of its CAD within 0.5 mm.
- **Smooth but inaccurate scans** (fused depth-camera scans, phone scans): Find All reads the noise as tiny and shatters the part into many small spheres and tori. Give it the scanner's accuracy: `scan.find_all {"tolerance": 0.3}` (or set `scan.tolerance {}` first, which Find All then uses as its least tolerance), then `scan.reconstruct {}`.
- **Clean less before Reconstruct:** Repair and Remove Floating help; Denoise and Fill Holes do not (on the T-LESS benchmark both made Reconstruct read worse). `scan.survey {}` suggests them only once Find All shows a fitted-surface route.
- **Refusals:** a build step the kernel refuses says why in words and what to try. Every later step then has no body, so undo to the last step that came back better and go on from there.
- **Organic part:** `scan.fit_patches {"point": [x,y,z], "radius": 30}` over a region, or `scan.free_form {"corners": [[…],[…],[…],[…]]}` for faces laid by hand. Read each reply's `rms`, `max` and `warning`. A surface that folds is kept, but it is wrong: use a smaller region or a higher `smoothing` instead.
- **By hand:** fit construction planes and axes at points on the scan with `scan.fit_plane {"point": [x,y,z]}` and `scan.fit_cylinder {"point": [x,y,z]}`; their replies give exact sizes. Cut the scan with `scan.section_sketch {"plane":"XY","offset":12}` to get a sketch of lines and arcs. Then model as `solidcraft-modeling` says, using those sizes as parameters.
- **Points on the scan:** every shape in `scan.find_all {}`'s reply has a `point` on it. Use those, not guesses, for `point` parameters.

## Verify
`scan.deviation {"body": "<model>"}` measures the scan against the model:
- `passes`: at least 95% of the measured points are within the tolerance.
- `max_above`: the scan stands outside the model, so the model lacks material there.
- `max_below`: the model stands outside the scan, so it has material it should not.
- `unmatched`: points too far from the model to count. A large share means a whole region is missing.

When the server is bridged to the app, `screenshot {}` shows the coloured map; `scan.deviation {"show": false}` takes it away. Only `passes: true` and a `measure {}` bbox that matches the scan's `own_size` count. A step that reported `ok` can still be millimetres off.

## Big scans
At more than 5 million triangles, `scan.survey {}` takes seconds, while `scan.find_all {}`, `scan.denoise {}` and `scan.remesh {}` take from tens of seconds to minutes. Read one region at a time with `scan.select {}` and `scan.reconstruct_selection {}`. Use `scan.reduce {}` only for fitting work you then check against the full scan. A call whose error starts `still running:` was not cancelled: `pending {}` waits for it.

## Read
| Command | Use | Reply worth reading |
|---|---|---|
| `scan.survey {}` | first look, changes nothing | `own_size`, `noise`, `smoothing`, `holes`, `floating`, `tangled_edges`, `sheet_like`, `shapes`, `next[{command, why, params}]` |
| `scan.find_all {}` | regions of the scan read as planes, cylinders, spheres, cones, tori or freeform (a view, no undo step) | `shapes[{index, kind, area, point, direction?, radius?, hole?}]`, `in_shapes`, `noise` |
| `scan.reconstruct {}` | what the shapes make: prisms, a turned part, holes, repeats, fillets (reads only) | `prisms`, `turned`, `holes`, `repeats`, `proposals[{command, params}]`, `note` |
| `scan.symmetry {}` | the mirror plane, kept as a construction plane | `mirror {point, normal, rms, coverage, symmetric}` |
| `scan.bases {}` | the ways the scan could stand, for `scan.stand_on_base {"base": n}` | `bases[{kind, normal, area, share}]` |

## Clean (each rewrites the scan, one undo step)
| Command | Use |
|---|---|
| `mesh.units {"units":"m"}` | file read in the wrong unit; refused once a feature follows the import |
| `scan.repair {}` | duplicate and unusable triangles, edges of three triangles, faces passing through each other |
| `scan.remove_floating {}` | pieces below `below` (default 0.02) of the largest one's triangles |
| `scan.denoise {}` | noise smoothed, sharp edges kept; default strength is the one the study suggests |
| `scan.fill_holes {}` | holes up to `max_edges` (default 300) closed with faired patches |
| `scan.reduce {"ratio": 0.25}` | fewer triangles; `max_error` stops it first |
| `scan.remesh {}` | an even mesh fused from the scan (`with` fuses other passes) |

## Place (each one Move feature)
`scan.stand_on_base {}` (or `{"point": [x,y,z]}` to stand on the flat face there), `scan.quick_align {}`, `scan.rest_on_grid {}`, `scan.center {}`, `scan.square_to_symmetry {}`, `scan.register {"to": "<other scan>"}`, `scan.align_all {}`, `scan.combine {}`, `scan.align_to_model {"model": "<body>"}`, `scan.align_pairs {}` (point pairs, when the automatic searches are fooled by a self-similar part).

## Build
| Command | Makes |
|---|---|
| `scan.extrude_it {"prism": 0}` | the prism's profile sketched off its walls and extruded, into `<scan> Reconstructed` |
| `scan.revolve_it {}` | the turned part's profile revolved |
| `scan.cut_it {"hole": 0}` | a round hole cut as deep as the scan has it |
| `scan.pattern_it {"hole": 0}` | that cut repeated as the holes repeat |
| `scan.fillet_it {}` | rounds measured off the scan, one Fillet per radius |
| `scan.chamfer_it {}` | chamfers read off the scan |
| `scan.fit_plane {"point": [x,y,z]}` | construction plane; reply `plane`, `rms`, `max` |
| `scan.fit_cylinder {"point": [x,y,z]}` | construction axis; reply `cylinder {point, axis, radius, diameter, length}` |
| `scan.fit_sphere {}`, `scan.fit_cone {}`, `scan.fit_torus {}` | the same for those shapes |
| `scan.fillet {}` | the radius of a round measured from the scan |
| `scan.section_sketch {"plane":"XY","offset":12}` | a sketch of lines and arcs fitted where the plane cuts the scan |
| `scan.sections_along_curve {}` | sections at stations along a curve |
| `scan.curve_on_scan {}` | a 3D spline laid on the scan through points |
| `scan.fit_patches {"point": [x,y,z], "radius": 30}` | a smooth surface over a region; `layout` grid, blocks or quads |
| `scan.free_form {"corners": [[…],[…],[…],[…]]}` | an editable surface face; grow it with `scan.add_face {}`, edit with `scan.edit_surface {}` |
| `scan.wrap_surface {}`, `scan.merge_surfaces {}`, `scan.slice_surface {}`, `scan.close_surface {}` | wrap round a tube-like part, join faces into one Free Form, slice into a solid, close an open surface |
| `scan.complete_by_mirroring {}` | the missing half filled from its mirror image; read its `warning` |
| `scan.flat_template {"outline":"scan"}` | a wall's outline and cut-outs as a sketch; `panel` for a straight sheet-cut panel |
| `scan.panel_proposals {}`, `scan.panel_export {}` | the panel's numbered items, its drawings (DXF, CSV, PDF) |
| `scan.refit {}` | a Free Form or Fit Patches surface fitted again to the scan as it is now, hand edits carried over |

## Check
| Command | Reply |
|---|---|
| `scan.tolerance {"tolerance": 0.2}` | the scan's tolerance, kept in the design |
| `scan.deviation {"body": "<model>"}` | `rms`, `mean`, `max_above` (material the model lacks), `max_below` (material it should not have), `within`, `passes` (95% within the tolerance), `measured`, `unmatched` |
| `inspect.surface_check {}` | folds, pinches and crossings of a surface |

## Known problems
- A metre or inch file inserted without `units` is 1000 or 25.4 times too small. `scan.survey {}` flags anything under 3 mm across.
- Find All reads less on very fine, noisy scans and on faceted CAD STLs. On the latter it works on a subdivided copy and takes longer. Reconstruct needs Find All's shapes, so a low `in_shapes` makes its proposals unreliable.
- Reconstruct reads multi-feature prismatic parts worst: shells, open-bottom parts and many small features can come out several millimetres off with material missing. Read what is left with `scan.select {"unexplained": …}` and `scan.reconstruct_selection {}`, or build from fits and sections.
- Two prisms stacked on one plane (a plate whose counterbores make a second prism below their floors) can be refused where the second meets the first (`the faces could not be joined where they meet`). Go on without it: the kernel refuses such joins of two prisms meeting on a shared plane, and nothing in the scan tools works round it yet.
- Reconstruct proposes nothing yet for shelled, drafted, lofted, swept or helical parts (a moulded housing, a blade, a bent bar, a spring): fit by hand and model, or Fit Patches.
- The kernel can refuse a join or cut onto a revolved body. The refusal names a reason and a fix; do the step on its own and try a different order.
- `scan.align_all {}` can misplace a pass of a blocky, self-similar part. Check `rms_after` and `matched`, and fall back to `scan.align_pairs {}`.
- A Fit Patches or Free Form surface on a deeply creviced organic region can fold. The reply says so in `warning`; use a smaller region.
- Mesh cleaning is not kept as separate timeline features: undo takes it back, but the design keeps only the cleaned mesh.
