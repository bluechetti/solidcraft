# Parity matrix

Made from `tests/parity/matrix.toml` and `tests/parity/quicksurface-spec.toml`; edit those, not this page.

Run: `a73b2306c`, 2026-11-18, clean worktree, branch scan-parity.

| Command | Result |
|---|---|
| `sctest -p solidcraft-scan` | 319 run, 319 passed |
| `sctest -p solidcraft-engine scan` | 187 run, 187 passed |
| `sctest -p solidcraft-ui-egui scan` | 131 run, 131 passed |

77 capabilities: 31 proven, 16 partial, 19 implemented-untested, 4 missing, 7 n/a.

"Proven" means the command does the job on a clean generated part in a test that passed in this run. It does not mean accurate under noise, and it says nothing about the other product's output.

| Id | Area | Capability | SolidCraft | Status | Note |
|---|---|---|---|---|---|
| QS-001 | import | Reads meshes from any scanner as STL, OBJ or PLY | `file.insert_mesh` | partial | STL proven by use; no test that STL, OBJ, PLY and 3MF give the same body |
| QS-002 | import | Reads PTX point clouds from long-range scanners | `pointcloud.insert` | missing | To build (port plan A3). Was: point clouds read PLY, XYZ and PTS only; no PTX reader |
| QS-003 | import | Loads a mesh of any size | `file.insert_mesh` | implemented-untested | cap is 250M triangles [code]; 28M measured in the lab [notes]; no test at 10M or more |
| QS-004 | mesh | Reduces the triangle count while keeping accuracy | `scan.reduce` | partial | U15; reported error is wrong at open rims |
| QS-005 | mesh | Fills holes | `scan.fill_holes` | partial | U8; some rings fail without a reason in the reply |
| QS-006 | mesh | Removes outliers (stray pieces) | `scan.remove_floating` | proven |  |
| QS-007 | mesh | Makes a watertight mesh for printing or reverse engineering | `scan.clean_up`, `scan.repair`, `scan.fill_holes`, `scan.remesh` | implemented-untested | no test asserts the result is closed and manifold |
| QS-008 | mesh | Removes stamps, markers and similar small features in one click | `scan.remove_markers` | proven |  |
| QS-009 | mesh | Trims a mesh with surfaces and makes 3D sketch curves from the cut edges | `scan.split`, `scan.curve_from_selection` | proven |  |
| QS-010 | mesh | Splits a mesh with a curve or profile sketched in 3D | `scan.split` | proven |  |
| QS-011 | mesh | Colours the scan mesh by curvature | `inspect.curvature_map` | implemented-untested | exists for models; whether it colours a mesh body is not verified |
| QS-012 | mesh | Trims a mesh with CAD primitives before surfacing | `scan.split` | proven |  |
| QS-013 | selection | Interactive selection of mesh regions that shapes are then fitted to | `scan.select`, `scan.select_inside_curve`, `scan.keep_selection` | proven |  |
| QS-014 | primitives | Extracts planes, cylinders, cones and spheres from selected mesh | `scan.fit_plane`, `scan.fit_cylinder`, `scan.fit_sphere`, `scan.fit_cone`, `scan.fit_torus` | partial | plane, cylinder, sphere proven; no command-level test of the cone fit |
| QS-015 | primitives | Finds the planes, cylinders and cones of a scan automatically | `scan.find_all` | proven | pose invariance not tested |
| QS-016 | primitives | Relations between extracted primitives: parallel, perpendicular, coincident, offset | none | missing | To build (port plan B5). Was: fits take no relation to another fit; Reconstruct squares directions internally only |
| QS-017 | primitives | Constrained best fit (a fit held to a direction or to another shape) | none | missing | To build (port plan B5). Was: no fit held to a given direction (only Wrap Surface takes axis x|y|z) |
| QS-018 | primitives | Reference lines and points for aligning | `scan.fit_cylinder`, `scan.fit_sphere`, `construct.axis`, `construct.point` | proven |  |
| QS-019 | align | Places the object in world coordinates from extracted primitives with an interactive coordinate system | `scan.stand_on_base`, `scan.square_to_symmetry`, `scan.center` | partial | U7; no one command that takes plane, axis and point as the three datums |
| QS-020 | align | Quick Align: one step onto the coordinate system | `scan.quick_align` | proven |  |
| QS-021 | align | Aligns scan to scan or scan to CAD from three or more matching points | `scan.align_pairs` | proven |  |
| QS-022 | align | Global fine alignment of two or more overlapping scans into one | `scan.align_all`, `scan.register`, `scan.combine` | partial | U10, F6; stops about 2 mm off on fused scans; rms misleads at low overlap |
| QS-023 | symmetry | Finds the symmetry plane, shows how well it fits and lets it be adjusted | `scan.symmetry`, `scan.square_to_symmetry`, `scan.complete_by_mirroring` | proven | F3: refuses cars unless a plane is given |
| QS-024 | symmetry | Symmetry plane fitted to a chosen region only | `scan.symmetry` | missing | To build: Symmetry scored on a selected or kept region. Was: takes a guide line or a plane, not a selected region |
| QS-025 | section | Interactive section planes aligned to world or CAD planes | `scan.section_sketch`, `sketch.fit_curves_to_section`, `inspect.section` | proven |  |
| QS-026 | section | Sections along a curve | `scan.sections_along_curve` | proven | these sections are not dimensioned |
| QS-027 | section | Several sections made at once, for lofting | `scan.sections_along_curve`, `scan.loft_it` | partial | U4; Loft It leaves raw spline sections; no 'N parallel sections' option on Section to Sketch |
| QS-028 | sketch2d | Constrained 2D sketching with dimensions over the section | `scan.section_sketch`, `sketch.auto_constrain` | proven |  |
| QS-029 | sketch2d | Offset and linear or circular patterns inside the 2D sketch | `sketch.offset`, `sketch.pattern.rectangular`, `sketch.pattern.circular` | implemented-untested | native sketch commands; their own tests were not part of this run |
| QS-030 | sketch2d | Deviation colours between the sketch and the section while sketching | `scan.section_sketch` | partial | the section's fit error is reported once; no live colour between a hand-drawn sketch and the section |
| QS-031 | sketch2d | Sketch assistant: profiles rebuilt from the mesh as best-fit lines, arcs and circles | `scan.section_sketch` | proven |  |
| QS-032 | sketch2d | Planar surface from a 2D sketch, snapping within a tolerance | `surface.boundary`, `surface.patch` | implemented-untested | native; not run here |
| QS-033 | sketch2d | Helix curve for threads: pitch measured, start, end and axis set | `scan.coil_it`, `solid.coil`, `solid.thread` | partial | a spring's pitch is read; no tool reads a thread's pitch off a scan |
| QS-034 | sketch3d | Free-form curves drawn directly on the mesh | `scan.curve_on_scan` | proven |  |
| QS-035 | sketch3d | Blend curves between 3D curves, used by fill and free-form surfaces | none | n/a | Out of scope by the owner's decision of 2026-11-18: a surfacing-only tool, to come with automatic surfacing. Was: 3D G2 blend curves were not ported (sketch.blend_curve is 2D) |
| QS-036 | sketch3d | Control of curvature and of continuity where 3D curves join | none | n/a | Out of scope by the owner's decision of 2026-11-18: a surfacing-only tool, to come with automatic surfacing. Was: no continuity setting where 3D curves join |
| QS-037 | sketch3d | 3D sketch curves from existing boundaries | `scan.curve_from_selection` | proven |  |
| QS-038 | surfacing | Extruded and revolved surfaces or solids, with draft and snapping | `scan.extrude_it`, `scan.revolve_it`, `solid.extrude`, `solid.revolve` | proven | U1 gear teeth, E2 open rims, E5 drafted corners are accuracy findings for Phase 3 |
| QS-039 | surfacing | Interactive draft extrusion with live draft, direction and depth | `scan.extrude_it`, `solid.extrude` | partial | draft and depth are parameters (proven); no UI scenario of the live drag |
| QS-040 | surfacing | Loft and blend with G0, G1 or G2 and section weighting | `surface.loft`, `solid.loft`, `surface.blend` | implemented-untested | surface.loft takes continuity g0..g3 per end [code]; not run here |
| QS-041 | surfacing | Sweep | `solid.sweep`, `surface.sweep` | implemented-untested | native; no scan workflow test uses a general sweep |
| QS-042 | surfacing | Pipe | `scan.pipe_it`, `solid.pipe` | proven |  |
| QS-043 | surfacing | Fit Surface over a selected area, extended for trimming, with deviation colours | `scan.fit_surface`, `surface.extend` | partial | fit proven; the untrimmed fit spans the area's rectangle but nothing asserts it reaches past the area for trimming |
| QS-044 | surfacing | A fitted surface re-edited later: degree, UV layout, continuity | `scan.edit_surface`, `scan.edit_net` | proven |  |
| QS-045 | surfacing | Fill Surface bounded by 3D sketch curves, following the scan, with chosen continuity to neighbours; closes holes | `scan.free_form`, `scan.close_surface`, `surface.patch` | partial | four-sided fill and closing openings proven; 3- and 5-or-more-sided G2 fill not ported |
| QS-046 | surfacing | Extend surfaces with tangent or curvature continuity | `surface.extend` | implemented-untested | native; not run here |
| QS-047 | surfacing | Automatic surfacing: one click covers the mesh with few NURBS patches | `scan.fit_patches`, `scan.wrap_surface` | n/a | Out of scope by the owner's decision of 2026-11-18: the parity claim covers mechanical parts; organic shapes are done by guided Free Form. Next project after mechanical parity (template layouts first). Was: Fit Patches covers a chosen region in a grid; nothing lays patches out over a whole scan by itself, and no G2 across patches is claimed or measured (auto layout was not ported) |
| QS-048 | surfacing | The automatic surface can be edited afterwards | none | n/a | Out of scope by the owner's decision of 2026-11-18: goes with QS-047. Was: depends on QS-047 |
| QS-049 | surfacing | Fillet radius read from the scan and made a true fillet | `scan.fillet`, `scan.radius`, `scan.fillet_it` | proven | E1c: spurious large rounds proposed on spokes |
| QS-050 | freeform | Free-form modelling with quad surfaces that snap to the mesh | `scan.free_form`, `scan.add_face`, `scan.mirror_free_form` | proven |  |
| QS-051 | freeform | Deviation shown live while a free-form surface is edited | `scan.free_form`, `scan.edit_surface` | implemented-untested | the dialogs' 30 UI tests passed; none asserts the live deviation readout against a known value |
| QS-052 | freeform | Free-form surfaces stay editable inside a parametric model | `scan.edit_surface`, `scan.refit` | proven |  |
| QS-053 | freeform | Thicken or offset surfaces into watertight solids | `surface.thicken`, `surface.offset` | proven |  |
| QS-054 | hybrid | Hybrid parametric history: prismatic features and free-form surfaces in one editable model | `timeline`, `parameters.change`, `Scan Clean steps` | proven |  |
| QS-055 | hybrid | Quick Solidify: reconstructed surfaces to a solid in few steps | `scan.merge_surfaces`, `scan.slice_surface`, `scan.close_surface` | proven |  |
| QS-056 | hybrid | A plug-in edition that works inside SOLIDWORKS | none | n/a | a plug-in for another CAD product; SolidCraft is the CAD, and the ground rules forbid driving commercial CAD. STEP export (QS-070) is the transfer |
| QS-057 | solid_ops | Mutual trim of surfaces | `surface.trim`, `surface.stitch` | implemented-untested | native; not run here on fitted surfaces |
| QS-058 | solid_ops | Booleans on solids, with colours and diagnostics that show what went wrong | `solid.combine`, `inspect.interference` | implemented-untested | native; U12: a box minus a faceted solid worked 6 of 28 times and gave a raw kernel error |
| QS-059 | solid_ops | Trim or split a solid with a solid, a surface body or a sketch | `solid.split_body`, `solid.split_face` | implemented-untested | native; not run here |
| QS-060 | solid_ops | Fillet and chamfer with a dragged radius and deviation to the mesh shown live | `scan.fillet`, `scan.radius`, `solid.chamfer` | partial | radius read from the scan (proven); no deviation that follows a dragged radius, and none for chamfers |
| QS-061 | solid_ops | Variable-radius and three-face fillets | `solid.fillet` | implemented-untested | type variable exists [code]; a three-face (full round) fillet was not found |
| QS-062 | solid_ops | Shell to a wall thickness with several faces removed | `scan.shell_it`, `solid.shell` | proven | U3: thickness not rounded |
| QS-063 | solid_ops | Draft on chosen faces about a pull direction | `solid.draft` | implemented-untested | native; not run here |
| QS-064 | solid_ops | Move or offset a face keeping its neighbours | `solid.offset_face`, `solid.press_pull` | implemented-untested | native; not run here |
| QS-065 | solid_ops | Linear and circular patterns, and mirror | `scan.pattern_it`, `solid.pattern.circular`, `solid.pattern.rectangular`, `solid.mirror` | proven | U2: no tooth count on gears |
| QS-066 | analysis | Deviation colour map of the CAD model against the mesh at any stage | `scan.deviation`, `scan.deviation_pin`, `inspect.compare` | proven | both ways since d8ce78e67 and its follow-up: agrees with the independent scorer within 0.00095 mm on 10 cases and fails all six known-wrong fixtures (lab: harness/parity); time on 1M and 10M not yet recorded |
| QS-067 | analysis | Pass or fail shown on each feature as it is made (deviation, curvature, draft) | `scan.*_it (reply `fit`)` | partial | each step reports better/same/worse on deviation; a 'worse' step stays unless undone; no curvature or draft cue per feature |
| QS-068 | analysis | Draft analysis for moulding | `inspect.draft` | implemented-untested | native; not run here |
| QS-069 | analysis | Flatten a surface to a 2D sketch, with strain shown | `scan.flatten`, `scan.roll_onto_scan`, `scan.flat_template` | proven | strain_rms and strain_max are in the reply [code]; strain is not painted |
| QS-070 | export | Exports surfaces or solids as STEP | `file.export_step`, `file.insert_step` | implemented-untested | native; never tested on a solid rebuilt from a scan |
| QS-071 | export | Exports IGES | none | n/a | Out of scope by the owner's decision of 2026-11-18: STEP is the export; every CAD system the vendor names reads it. Was: no IGES reader or writer in the repo |
| QS-072 | export | Exports a watertight mesh for printing | `file.export_stl`, `file.export_3mf`, `file.export_obj` | implemented-untested | native; not run here on a cleaned scan |
| QS-073 | export | Exports a flattened outline as DXF | `sketch.export_dxf`, `scan.panel_export` | partial | Export Panel proven; DXF of a Flat Pattern not tested |
| QS-074 | ai | One step from a suitable mechanical scan to editable parametric CAD: feature tree, sketches, dimensions | `scan.survey`, `scan.reconstruct`, `scan.reconstruct_rest` | partial | E1, U1, E2; works on clean generated parts; one refused step stops the dialog's OK; T-LESS 05/18/27 read no prism |
| QS-075 | ai | Driven by the user's own AI assistant over the Model Context Protocol | `MCP server`, `prompt scan_to_cad`, `skill solidcraft-scan-to-cad` | implemented-untested | the mcp crate's tests were not in this run; no scripted end-to-end client test |
| QS-076 | ai | Recognises holes, fillets, chamfers, revolved features and drafts | `scan.hole_it`, `scan.fillet_it`, `scan.chamfer_it`, `scan.revolve_it`, `scan.extrude_it` | proven |  |
| QS-077 | ai | Time saved by the AI tools | none | n/a | a time-saving claim with no public baseline cannot be compared; our wall time per workflow is recorded in Phase 4 |
