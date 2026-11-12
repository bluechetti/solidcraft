//! Scan to CAD (#8615): the tools of the Scan workspace. Placing a scan (standing it on a base,
//! resting it on the grid, centring it, laying it along its own axes, squaring it to its mirror
//! plane) works on a mesh body and is kept as an ordinary Move feature, and a mirror plane found
//! is kept as an ordinary construction plane, so a design opens anywhere. The tools not built yet are
//! registered with their place, icon and parameters so the tab shows its whole layout; the tab's
//! other working tools (Insert Mesh, Insert Point Cloud, Convert Mesh, Fit Curves to Mesh Section,
//! Section Analysis, Compare Bodies) live with their own areas.

use serde_json::{Value, json};
use solidcraft_geom::Vec3Geom as _;
use solidcraft_scan::Scan;
use solidcraft_scan::base::{self, Base};
use solidcraft_scan::fit::{self, Fitted, Kind, Shape};
use solidcraft_scan::place::{self, Rigid};
use solidcraft_scan::symmetry::{self, Guide, Mirror};

use super::CommandSpec;
use crate::params::{bad, str_};
use crate::schema::scan::{
    ScanAddFaceParams, ScanAddFaceResult, ScanBase, ScanBasesParams, ScanBasesResult, ScanCloseSurfaceParams, ScanCloseSurfaceResult,
    ScanCompleteParams, ScanCompleteResult, ScanCurveParams, ScanCurveResult, ScanDeviationParams, ScanDeviationResult, ScanEditNetParams,
    ScanEditNetResult, ScanEditSurfaceParams, ScanEditSurfaceResult, ScanFitParams, ScanFitResult, ScanFitSurfaceParams, ScanFitSurfaceResult,
    ScanFittedCone, ScanFittedCylinder, ScanFittedPlane, ScanFittedSphere, ScanFittedTorus, ScanFloatingParams, ScanFloatingResult,
    ScanFreeFormParams, ScanFreeFormResult, ScanHolesParams, ScanHolesResult, ScanMergeSurfacesParams, ScanMergeSurfacesResult, ScanMirror,
    ScanMirrorFreeFormParams, ScanMirrorFreeFormResult, ScanPlaceParams, ScanPlaceResult, ScanReduceParams, ScanReduceResult, ScanRepairParams,
    ScanRepairResult, ScanSectionParams, ScanSectionResult, ScanSectionsAlongParams, ScanSectionsAlongResult, ScanSliceSurfaceParams,
    ScanSliceSurfaceResult, ScanSquareParams, ScanStandParams, ScanSymmetryParams, ScanSymmetryResult, ScanWrapSurfaceParams, ScanWrapSurfaceResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Sel, Session};
use solidcore_record::Vec3;

const BASES_CONTRACT: TypedContract<ScanBasesParams> = TypedContract::new();
const PLACE_CONTRACT: TypedContract<ScanPlaceParams> = TypedContract::new();
const STAND_CONTRACT: TypedContract<ScanStandParams> = TypedContract::new();
const SYMMETRY_CONTRACT: TypedContract<ScanSymmetryParams> = TypedContract::new();
const SQUARE_CONTRACT: TypedContract<ScanSquareParams> = TypedContract::new();
const FIT_CONTRACT: TypedContract<ScanFitParams> = TypedContract::new();
const DEVIATION_CONTRACT: TypedContract<ScanDeviationParams> = TypedContract::new();
const SURFACE_CONTRACT: TypedContract<ScanFitSurfaceParams> = TypedContract::new();
const COMPLETE_CONTRACT: TypedContract<ScanCompleteParams> = TypedContract::new();
const EDIT_SURFACE_CONTRACT: TypedContract<ScanEditSurfaceParams> = TypedContract::new();
const CURVE_CONTRACT: TypedContract<ScanCurveParams> = TypedContract::new();
const EDIT_NET_CONTRACT: TypedContract<ScanEditNetParams> = TypedContract::new();
const FREE_FORM_CONTRACT: TypedContract<ScanFreeFormParams> = TypedContract::new();
const ADD_FACE_CONTRACT: TypedContract<ScanAddFaceParams> = TypedContract::new();
const MIRROR_FREE_FORM_CONTRACT: TypedContract<ScanMirrorFreeFormParams> = TypedContract::new();
const MERGE_SURFACES_CONTRACT: TypedContract<ScanMergeSurfacesParams> = TypedContract::new();
const CLOSE_SURFACE_CONTRACT: TypedContract<ScanCloseSurfaceParams> = TypedContract::new();
const SLICE_SURFACE_CONTRACT: TypedContract<ScanSliceSurfaceParams> = TypedContract::new();
const WRAP_SURFACE_CONTRACT: TypedContract<ScanWrapSurfaceParams> = TypedContract::new();
const FLOATING_CONTRACT: TypedContract<ScanFloatingParams> = TypedContract::new();
const HOLES_CONTRACT: TypedContract<ScanHolesParams> = TypedContract::new();
const REDUCE_CONTRACT: TypedContract<ScanReduceParams> = TypedContract::new();
const REPAIR_CONTRACT: TypedContract<ScanRepairParams> = TypedContract::new();
const SECTION_CONTRACT: TypedContract<ScanSectionParams> = TypedContract::new();
const SECTIONS_ALONG_CONTRACT: TypedContract<ScanSectionsAlongParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.repair", "Repair Mesh", crate::checked!("scan.repair", clean::repair))
        .at("SCAN", "CLEAN")
        .icon("mesh_body")
        .contract(&REPAIR_CONTRACT)
        .output(output_schema_for::<ScanRepairResult>)
        .params("body?: mesh body (default: the selected or only one); crossings?: bool (default true: faces passing through others they share no edge with taken off with the ring round them, the holes left closed with smooth patches): copies of a triangle taken off, an edge three or more faces meet along left with the two that continue each other best, slivers flipped away (equal points, collapsed triangles and the winding are put right when a scan is built); the scan written back in its Insert Mesh feature (one undo step); a scan that needs none is told so → body, duplicates, non_manifold, slivers, crossings, crossing_faces, refilled, triangles, feature"),
    CommandSpec::new("scan.fill_holes", "Fill Holes", crate::checked!("scan.fill_holes", clean::fill_holes))
        .at("SCAN", "CLEAN")
        .icon("surface_patch")
        .contract(&HOLES_CONTRACT)
        .output(output_schema_for::<ScanHolesResult>)
        .params("body?: mesh body (default: the selected or only one); max_edges?: the largest hole to close, in edges round it (3…5000, default 300); fair?: bool (default true: each patch carries the surface's shape across; false leaves it flat): the holes closed with patches as fine as the scan round them (each piece's outer rim is not a hole), the scan written back in its Insert Mesh feature (one undo step) → body, holes, filled, failed, larger, added_triangles, unfillable_edges, feature"),
    CommandSpec::new("scan.reduce", "Reduce Mesh", crate::checked!("scan.reduce", clean::reduce))
        .at("SCAN", "CLEAN")
        .icon("scale")
        .contract(&REDUCE_CONTRACT)
        .output(output_schema_for::<ScanReduceResult>)
        .params("body?: mesh body (default: the selected or only one); ratio?: share of the triangles to keep (0…1) or triangles?: how many to keep (default: half); max_error?: length a collapse may move the surface by, root mean square (stops first if reached); keep_rim?: bool (default true: the scan's open edges stay where they are): edges collapsed cheapest first (quadric errors), never folding the surface, the scan written back in its Insert Mesh feature (one undo step) → body, before, triangles, collapses, stopped (target|error|no_more_collapses), largest_error, feature"),
    CommandSpec::new("scan.remove_floating", "Remove Floating Pieces", crate::checked!("scan.remove_floating", clean::remove_floating))
        .at("SCAN", "CLEAN")
        .icon("delete")
        .contract(&FLOATING_CONTRACT)
        .output(output_schema_for::<ScanFloatingResult>)
        .params("body?: mesh body (default: the selected or only one); below?: share of the largest piece's triangles (0…0.5, default 0.02; never fewer than 2): the pieces of the scan (triangles joined through their corners) smaller than that taken off, the scan written back in its Insert Mesh feature (one undo step) → body, pieces, removed, removed_triangles, triangles, feature"),
    CommandSpec::new("scan.bases", "Scan Bases", crate::checked!("scan.bases", bases))
        .icon("list")
        .noundo()
        .contract(&BASES_CONTRACT)
        .output(output_schema_for::<ScanBasesResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one) → body, bases: [{kind: flat|resting|axis, normal, anchor, area, share, tip_degrees, rms}], steadiest first, at most 8"),
    CommandSpec::new("scan.stand_on_base", "Stand on Base", crate::checked!("scan.stand_on_base", stand_on_base))
        .at("SCAN", "ALIGN")
        .icon("align")
        .contract(&STAND_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); point?: [x,y,z] on the scan (grow the flat face there and stand on it; a curved wall is refused; default: the point picked on the scan); base?: index into scan.bases; with neither, the steadiest base, or pressed again the next one → body, moved, base, base_index, base_count, feature (the Move)"),
    CommandSpec::new("scan.rest_on_grid", "Rest on Grid", crate::checked!("scan.rest_on_grid", rest_on_grid))
        .at("SCAN", "ALIGN")
        .icon("move")
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one): up or down until its lowest point is at Z = 0 → body, moved, feature (the Move)"),
    CommandSpec::new("scan.center", "Center on Origin", crate::checked!("scan.center", center))
        .at("SCAN", "ALIGN")
        .icon("origin")
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one): sideways until the middle of its box is over the origin, its height kept → body, moved, feature (the Move)"),
    CommandSpec::new("scan.quick_align", "Quick Align", crate::checked!("scan.quick_align", quick_align))
        .at("SCAN", "ALIGN")
        .icon("axis")
        .contract(&PLACE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one): laid along its own directions, longest along X and thinnest along Z, the middle of its surface at the origin; a scan too round to have a longest direction is refused → body, moved, feature (the Move)"),
    CommandSpec::new("scan.symmetry", "Find Symmetry", crate::checked!("scan.symmetry", find_symmetry))
        .at("SCAN", "ALIGN")
        .icon("mirror")
        .contract(&SYMMETRY_CONTRACT)
        .output(output_schema_for::<ScanSymmetryResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); guide?: {from, to: [x,y,z], toward?: [x,y,z] (the way you looked, default [0,0,1])}: a line along the rough mirror plane, to find that one; plane?: {point, normal: [x,y,z]} to keep as it is; keep?: bool (default true: kept as a construction plane `Symmetry Plane`, `Symmetry Plane 2`, …, to sketch on); a plane given or guided to is used however much matches, the best one found unasked is refused when the scan is no mirror image about it → body, mirror: {point, normal, rms, coverage, symmetric}, plane (the construction plane)"),
    CommandSpec::new("scan.square_to_symmetry", "Square to Symmetry", crate::checked!("scan.square_to_symmetry", square_to_symmetry))
        .at("SCAN", "ALIGN")
        .icon("c_symmetry")
        .contract(&SQUARE_CONTRACT)
        .output(output_schema_for::<ScanPlaceResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); guide?: {from, to: [x,y,z], toward?: [x,y,z]}; plane?: {point, normal}: the smallest turn that lays the mirror plane along YZ, then sideways to X = 0, so the origin's YZ plane is the mirror plane; a plane given or guided to is used however much matches, the best one found unasked is refused when the scan is no mirror image about it → body, moved, mirror, feature (the Move)"),
    CommandSpec::new("scan.complete_by_mirroring", "Complete by Mirroring", crate::checked!("scan.complete_by_mirroring", complete_by_mirroring))
        .at("SCAN", "ALIGN")
        .icon("scan_mirror_fill")
        .contract(&COMPLETE_CONTRACT)
        .output(output_schema_for::<ScanCompleteResult>)
        .params("body?: mesh body (default: the selected mesh body, else the design's only one); plane?: {point, normal: [x,y,z]} to reflect in (default: found near guide, else the best found; a one-sided scan is not refused for matching little, but `warning` says so); guide?: {from, to, toward?}; reach?: length a reflected triangle must lie from the scan to count as missing (default 1.5 typical edges or 3 × the plane's mismatch): the reflected triangles where the scan has nothing, as an amber mesh body `<scan> Mirror Fill` beside it (one undo step); a scan missing nothing is refused → body, fill, mirror, pieces, triangles, area, joined, reach, warning?, feature"),
    CommandSpec::new("scan.fit_plane", "Fit Plane", crate::checked!("scan.fit_plane", fit_plane))
        .at("SCAN", "FIT")
        .icon("plane")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan (default: the point picked on it; the region grows from it); tolerance?: mm (default 0.15 % of the scan's size); keep?: bool (default true: kept as a construction plane) → body, plane: {point, normal}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_cylinder", "Fit Cylinder", crate::checked!("scan.fit_cylinder", fit_cylinder))
        .at("SCAN", "FIT")
        .icon("cylinder")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan (default: the point picked on it; the region grows from it); tolerance?: mm (default 0.15 % of the scan's size); keep?: bool (default true: kept as a construction axis) → body, cylinder: {point, axis, radius, diameter, length}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_sphere", "Fit Sphere", crate::checked!("scan.fit_sphere", fit_sphere))
        .at("SCAN", "FIT")
        .icon("sphere")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan (default: the point picked on it; the region grows from it); tolerance?: length (default 0.15 % of the scan's size); keep?: bool (default true: its centre kept as a construction point) → body, sphere: {center, radius, diameter}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_cone", "Fit Cone", crate::checked!("scan.fit_cone", fit_cone))
        .at("SCAN", "FIT")
        .icon("cone")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("point?: [x,y,z] on the scan (default: the point picked on it); body?: mesh body (default: the picked one, else the selected or only one); tolerance?: length (how far a triangle may be from the cone and still be on it; default 0.15% of the scan's size); keep?: bool (default true): a cone fitted to the slanted round wall round the point (a chamfer, a countersink, a drafted boss), kept as a construction axis along it → cone {apex, axis, half_angle}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_torus", "Fit Torus", crate::checked!("scan.fit_torus", fit_torus))
        .at("SCAN", "FIT")
        .icon("torus")
        .contract(&FIT_CONTRACT)
        .output(output_schema_for::<ScanFitResult>)
        .params("point?: [x,y,z] on the scan (default: the point picked on it); body?: mesh body (default: the picked one, else the selected or only one); tolerance?: length (default 0.15% of the scan's size); keep?: bool (default true): a torus fitted to the round round the point (a fillet along a circular edge), kept as a construction axis through its centre → torus {center, axis, major, minor}, triangles, area, rms, max, feature"),
    CommandSpec::new("scan.fit_surface", "Fit Surface", crate::checked!("scan.fit_surface", fit_surface))
        .at("SCAN", "FREEFORM")
        .icon("surface")
        .contract(&SURFACE_CONTRACT)
        .output(output_schema_for::<ScanFitSurfaceResult>)
        .params("body?: mesh body (default: the picked scan's, else the selected or only mesh body); point?: [x,y,z] on the scan, the middle of the area (default: the point picked on it); radius?: length (default a tenth of the scan's size); controls?: 4…30 control points each way (default 8); smoothing?: 0…1 (default 0.1): a smooth B-spline surface over the area, fitted in all three directions (each point moved to its foot on it between rounds), kept as a surface body (one undo step); an area that wraps round is refused; a surface that folds, pinches or passes through itself is kept and said so (warning) → body, surface, rms, max, points, controls, folds, pinches, crossings, warning?, feature"),
    CommandSpec::new("scan.edit_surface", "Edit Surface", crate::checked!("scan.edit_surface", surface::edit_surface))
        .at("SCAN", "FREEFORM")
        .icon("scan_edit_surface")
        .contract(&EDIT_SURFACE_CONTRACT)
        .output(output_schema_for::<ScanEditSurfaceResult>)
        .params("surface?: the surface body (default: the selected or only one Fit Surface made; moved since is fine, reshaped is refused); dots?: [{at: [x,y,z] (the point on the surface nearest it) | point: [u, v] with face?: n, to: [x,y,z] | along: length}] points on the surface moved (the surface at that control point's place on it, the points spread evenly over each face, runs through where it goes; the way to edit a Free Form of several faces), neighbours?: hold (default: the surface still runs through every other point on it) | follow (the points round it go with it); faces?: [{count, knots, points, degree?}, …] every face as it now is, given alone; points?: the whole control net [[x,y,z], …], u fastest; moves?: [{point: [u, v], to: [x,y,z] | along: length along the surface's normal}]; hold?: [[u, v], …] points kept where they are when refitting (besides those moved); net?: {count, knots, points, degree?} a whole new net, given alone; refit?: none|around|all (default none: the ring of two round each point moved or held, or every point not moved or held, fitted to the scan again); scan?: mesh body to refit to (default the selected or only one); reach?: length a scan point may be from the surface to count as under it (default a tenth of the net's size, at least 1 mm); falloff?: length (default 0): the points within it of a point moved or slid go with it, less the further they are, and are held when refitting; slide?: {points: [[u, v], …], u?: length, v?: length} the points slid about that far along the surface instead of moved through space (not with moves or points); crease?: [{at: [x,y,z] (the shared edge nearest it) | edge: [[x,y,z], [x,y,z]] (its ends), chain?: bool (also the edges it runs on into through corners where four faces meet as a grid's do)}] shared edges of a Free Form made sharp: the faces no longer held tangent across each and the points near it fitted to the scan again, so they take the scan's edge (the design keeps a crease where the faces still meet smoothly); uncrease?: the same made smooth again (both given alone); brush?: {points: [[x,y,z], …], radius: length, strength?: 0…1 (default 0.5), passes?: 1…100 (default 4), snap?: bool (default true)} a smoothing brush dabbed at each place: the points on the surface within the radius evened out, more toward the middle, kept on the scan (off: faired), given alone; smooth?: true | {strength?: 0…1 (default 0.5), passes?: 1…20 (default 1), snap?: bool (default true)} the whole surface smoothed, its own corners held, given alone; delete_face?: {at: [x,y,z]} the face of a Free Form nearest it taken away (not the last, nor one between others), given alone; weld?: {at: [x,y,z], onto: [x,y,z]} the corner on the Free Form's edge nearest `at` dropped onto the other one nearest `onto`: one point from then on, and two open sides whose ends then meet sewn into one shared edge (held tangent where the faces go on from each other), given alone; keep_half?: {side: [x,y,z], plane?} the faces on the far side of the symmetry plane (default the kept one) from `side` taken away, Symmetry dropped, given alone; kept_edges?: {creases?: [[[x,y,z], [x,y,z]], …], smooths?: […]} with `faces`: the shared edges creased or held smooth by hand, kept in the design; symmetry?: YZ|XZ|XY|construction plane|{point, normal}|none, with an edit of a Free Form of several faces: the plane it is kept a mirror image in, each change made on both sides (default: the plane the design keeps, from Mirror Free Form or Edit Surface's Symmetry); the design keeps a Free Form's hand-set creases and its symmetry plane: one undo step → surface, controls [u, v], moved, carried, refitted, scan, points, rms, max, faces, creases, sewn?, feature"),
    CommandSpec::new("scan.curve_on_scan", "Curve on Scan", crate::checked!("scan.curve_on_scan", curve::curve_on_scan))
        .at("SCAN", "SKETCH")
        .icon("spline")
        .contract(&CURVE_CONTRACT)
        .output(output_schema_for::<ScanCurveResult>)
        .params("body?: mesh body (default: the selected or only one); points: [[x,y,z], …] clicked along the scan, in order (2 or more, 3 for closed); closed?: bool; tolerance?: length the curve may stray from the line laid on the scan (default: the scan's typical triangle side, at most 0.1% of its size); sketch?: id|name of a 3D sketch (default: the 3D sketch being edited, else `Scan Curves`, made the first time): the smooth line through the points laid on the scan, kept as a 3D sketch spline through the fewest points within the tolerance (one undo step); a line across a hole is refused, naming the span → body, curve, sketch, closed, points, fit_points, length, max, off_scan"),
    CommandSpec::new("scan.section_sketch", "Section to Sketch", crate::checked!("scan.section_sketch", section::section_sketch))
        .at("SCAN", "SKETCH")
        .icon("fit_section")
        .contract(&SECTION_CONTRACT)
        .output(output_schema_for::<ScanSectionResult>)
        .params("body?: mesh body (default: the selected or only one); plane?: as sketch.create's (default XY; used when no sketch is being edited, else that sketch's plane); offset?: length; tolerance?: how far a piece may stray from the cut (default: three times the cut's noise, at least 0.1 mm and at most 1 % of the cut's size): where the plane cuts the scan, as lines, arcs and circles each fitted to its points (the noise averaged, not traced), end to end, smooth joins tangent, in the sketch being edited or a new sketch `Section` (one undo step) → body, sketch, outlines, lines, arcs, circles, tangents, tolerance"),
    CommandSpec::new("scan.sections_along_curve", "Sections Along Curve", crate::checked!("scan.sections_along_curve", section::sections_along_curve))
        .at("SCAN", "SKETCH")
        .icon("loft")
        .contract(&SECTIONS_ALONG_CONTRACT)
        .output(output_schema_for::<ScanSectionsAlongResult>)
        .params("body?: mesh body (default: the selected or only one); sketch?: id|name of the 3D sketch holding the curve (default: the last `Scan Curves`); curves?: [ids end to end in order] (default: its last curve); count?: 1…60 sections (default 8: end to end of an open curve, round a closed one); every?: bool (default false: the loop round the curve with the loops inside it, or the one nearest it); tolerance?: how far a piece may stray from the cut (default: three times the cut's noise, at least 0.1 mm and at most 1 % of the cut's size): planes square to the curve at even steps cut the scan, each section a sketch `Curve Section n` on its plane of lines, arcs and circles fitted to the cut, the planes turned with the curve and never about it so they loft in order (one undo step); a plane that cuts nothing near the curve is counted, not refused → body, sketches, missed, outlines, lines, arcs, circles, tangents, length, tolerance"),
    CommandSpec::new("scan.free_form", "New Free Form", crate::checked!("scan.free_form", free_form::new_free_form))
        .at("SCAN", "FREEFORM")
        .icon("scan_free_form")
        .contract(&FREE_FORM_CONTRACT)
        .output(output_schema_for::<ScanFreeFormResult>)
        .params("corners: [[x,y,z] × 4] in order round the first face (or curves: [[[x,y,z], …] × 4] the face's four sides, any order and direction, meeting end to end, as Curve on Scan points them: its edges follow them and its inside is fitted to the scan, count? points each way); fit?: off|follow|overall|flat (default follow: fitted to the scan under it; overall: the places far off it left out, so it spans openings and ribs; flat: exactly flat, in the one plane fitted to the scan under it with the places far off it left out, so a panel or wall need not follow every bump; off: flat over the corners only if they are in one plane); plane?: YZ|XZ|XY|construction plane name|{point, normal} (lay the face flat on that plane instead of fitting one: the corners put onto it, no scan needed; fit is then flat); count?: 4…12 points each way (default 4); stiffness?: 0…1 (default 0); reach?: length (snap distance: the scan met no farther than this); body?: the scan (default: the selected or only one): a surface of its own, a cubic net, to edit with Edit Surface and Edit Net (one undo step) → body, surface, controls, places, met, kept, rms, max (how far the scan under it is from it), feature"),
    CommandSpec::new("scan.add_face", "Add Face", crate::checked!("scan.add_face", free_form::add_face))
        .icon("scan_free_form")
        .contract(&ADD_FACE_CONTRACT)
        .output(output_schema_for::<ScanAddFaceResult>)
        .params("edge: [[x,y,z] × 2] at or near the ends of an open edge of the Free Form; far: [[x,y,z] × 2] the new face's far corners (either order; a corner on a point of the Free Form is that point); rows?: 4…12 points across (default 4); reach?: length (the scan met no farther than this); surface?: the Free Form (default: the selected one, else the one made last); body?: the scan; symmetry?: YZ|XZ|XY|construction plane|{point, normal}|none (default: the plane the design keeps for the Free Form): the face's mirror image added too (`twin`); a face added on that edge, sharing it, tangent to the face it grows from, a side that runs along another open edge shared with it too, fitted to the scan on its own and then held tangent to the faces it goes on from smoothly (a side where the scan turns sharply is left a crease, `sharp`); a face that would have a flat corner (one face over a smooth rim) is refused; the Free Form stays one surface body of several faces, a solid once it closes, edited with Edit Surface (one undo step) → surface, faces, face, twin?, controls, body, rms, max, kink (where held tangent), irregular, sharp, feature"),
    CommandSpec::new("scan.mirror_free_form", "Mirror Free Form", crate::checked!("scan.mirror_free_form", free_form::mirror_free_form))
        .icon("scan_mirror_fill")
        .contract(&MIRROR_FREE_FORM_CONTRACT)
        .output(output_schema_for::<ScanMirrorFreeFormResult>)
        .params("plane: YZ|XZ|XY | a construction plane's name (Find Symmetry's `Symmetry Plane`) | {point, normal}; gap?: length (how far from the plane an edge may be to be put onto it, default a tenth of the net's size, at least 1 mm); surface?: the Free Form (default: the selected one, else the one made last): a half modelled on one side of the plane mirrored and joined to its image on the plane, its edges within the gap put onto it, one Free Form that Edit Surface's Symmetry keeps a mirror image (the design keeps the plane: later edits and Add Face make every change on both sides); one that is a mirror image already is left as it is (how: already); one on both sides that is not, or none of whose edges reaches the plane, is refused (one undo step) → surface, faces, how, added, onto, pairs, on_plane, kink, feature"),
    CommandSpec::new("scan.wrap_surface", "Wrap Surface", crate::checked!("scan.wrap_surface", free_form::wrap_surface))
        .at("SCAN", "FREEFORM")
        .icon("surface_revolve")
        .contract(&WRAP_SURFACE_CONTRACT)
        .output(output_schema_for::<ScanWrapSurfaceResult>)
        .params("point?: [x,y,z] on the part (default: the point picked on the scan); reach?: length (how far round the point the part reaches, default a fifth of the scan's size); axis?: fitted|x|y|z (default fitted: the cylinder fitted to the part; x, y, z the world's, its line and radius fitted across the part); details?: low|medium|fine (4, 8 or 14 points the shorter way, default medium; the longer way as many as keep the cells square, up to 40); arc?: angle (how far round the net goes about the part's middle, default the part's own reach; the whole way round is left open at a seam); stiffness?: 0…1 (default 0); body?: the scan: a Free Form wrapped on a tube-like part, a surface of its own to edit with Edit Surface (one undo step); a surface that folds, pinches or passes through itself (a net following rims and ribs it is too coarse for) is kept and said so (warning: raise stiffness) → body, surface, controls, point, axis, radius, off, places, met, rms, max (how far the scan under it is from it), seam? (how far apart the ends of a net that goes the whole way round are: it is open there), folds, pinches, crossings, warning?, feature"),
    CommandSpec::new("scan.merge_surfaces", "Merge Surfaces", crate::checked!("scan.merge_surfaces", free_form::merge_surfaces))
        .at("SCAN", "FREEFORM")
        .icon("surface_stitch")
        .contract(&MERGE_SURFACES_CONTRACT)
        .output(output_schema_for::<ScanMergeSurfacesResult>)
        .params("surfaces?: [surface body names] (default: the selected ones, else every surface Fit Surface, New Free Form or Wrap Surface made; one alone is closed at its own seam, as a wrap that goes the whole way round); gap?: length (how far apart two sides may be on average to be made one, default a tenth of the shortest side); stitch?: bool (default true: made one Free Form, kept in the first surface's feature and edited with Edit Surface, a solid when its faces close; the other surfaces are removed from here on): each side that runs along another's within the gap made one curve (the nets' knots matched, corners and points put half way between), a Free Form of several faces taken whole, one undo step → surfaces, seams, gap, body, faces, feature"),
    CommandSpec::new("scan.close_surface", "Close Surface", crate::checked!("scan.close_surface", close::close_surface))
        .at("SCAN", "FREEFORM")
        .icon("surface_patch")
        .contract(&CLOSE_SURFACE_CONTRACT)
        .output(output_schema_for::<ScanCloseSurfaceResult>)
        .params("surface?: the surface body (default: the selected one, else the one made last); close?: bool (default true; false only lists the openings and changes nothing); continuity?: g0|g1 (default g0: how a patch meets the faces round its opening): each loop of free edges closed with a patch and all sewn, a solid when nothing else is open; an opening a patch cannot close is said with where it is, nothing changed; one undo step → body, loops (edges, at, size), caps, open, solid, volume?"),
    CommandSpec::new("scan.slice_surface", "Slice Surface", crate::checked!("scan.slice_surface", slice::slice_surface))
        .at("SCAN", "FREEFORM")
        .icon("surface_trim")
        .contract(&SLICE_SURFACE_CONTRACT)
        .output(output_schema_for::<ScanSliceSurfaceResult>)
        .params("plane: XY|XZ|YZ|plane name|{origin, normal}; keep?: front|back|[x,y,z] (default front: where the plane's normal points); cap?: bool (default true: each loop of the cut closed with a flat face and sewn in, a solid when nothing else is open; refused when the plane does not cut right round the surface); surface?: the surface body (default: the selected one, else the one made last): Trim, Patch and Stitch in one undo step → body, caps, solid"),
    CommandSpec::new("scan.edit_net", "Edit Net", crate::checked!("scan.edit_net", surface::edit_net))
        .icon("grid")
        .contract(&EDIT_NET_CONTRACT)
        .output(output_schema_for::<ScanEditNetResult>)
        .params("surface?: the surface body (default: the selected one, else the one made last); op: refine (in?: u|v|both) | split | duplicate (after?: bool, default true) | remove | crease | uncrease | extend (distance?: length, default 10 mm); line?: {row: j} | {column: i}: the net changed as a whole (refine and split leave the surface as it was; a crease makes the surface run sharp along the line, needing three lines each side), one undo step → surface, controls, line, creases, feature"),
    CommandSpec::new("scan.deviation", "Scan Deviation", crate::checked!("scan.deviation", deviation))
        .at("SCAN", "CHECK")
        .icon("curvature_map")
        .noundo()
        .contract(&DEVIATION_CONTRACT)
        .output(output_schema_for::<ScanDeviationResult>)
        .params("scan?: mesh body (default: the selected or only one); body?: the model: a solid or surface body, or a mesh body (a CAD model that came as an STL) (default: the selected or only solid or surface body, else with none the only other mesh body); tolerance?: length (default 0.1 mm); limit?: length (default 0.5 mm); show?: bool (false takes the map away): colours the scan by its distance from the model in the viewport, a view and not an undo step → shown, scan, body, rms, mean, max_above, max_below, within, measured, unmatched, tolerance, limit"),
];

/// A move smaller than this (mm, at the scan's reach) leaves the scan where it is.
const NEGLIGIBLE: f64 = 1e-3;

/// A scan's triangles, copied out of the evaluated model so the session can then move it.
struct Mesh {
    body: String,
    positions: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
}

impl Mesh {
    fn scan(&self) -> Scan<'_> {
        Scan::new(&self.positions, &self.triangles)
    }

    /// The farthest a point of the scan is from the origin, for judging a move negligible.
    fn reach(&self) -> f64 {
        self.positions.iter().map(|p| p.len()).fold(1.0, f64::max)
    }
}

/// The mesh body a scan command works on: `body`, else the one selected mesh body, else the
/// design's only mesh body.
fn mesh(s: &Session, p: &Value, cmd: &str) -> Result<Mesh> {
    let name = mesh_name(s, p, cmd)?;
    let state = s.model.state();
    let body = state.body(&name).ok_or_else(|| bad(cmd, format!("no body `{name}`")))?;
    let (positions, triangles) = body.body.triangle_mesh().ok_or_else(|| bad(cmd, format!("`{name}` is not a scan (a mesh body)")))?;
    Ok(Mesh { body: name, positions: positions.to_vec(), triangles: triangles.to_vec() })
}

/// The scan a scan command would work on with no `body` given (`body`, else the selected or only
/// mesh body), for a dialog to say which and how big.
pub fn mesh_body_name(s: &Session, body: Option<&str>, cmd: &str) -> Result<String> {
    mesh_name(s, &json!({ "body": body }), cmd)
}

/// The name of the mesh body [`mesh`] would take, without copying its triangles.
fn mesh_name(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    let state = s.model.state();
    let named = str_(p, "body").filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let name = match named {
        Some(name) => name,
        None => {
            let mut selected: Vec<&str> = s
                .selection
                .iter()
                .filter_map(|x| match x {
                    Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .filter(|name| state.body(name).is_some_and(|b| b.body.is_mesh()))
                .collect();
            // Several points picked on one scan are one scan.
            selected.sort_unstable();
            selected.dedup();
            let meshes: Vec<&str> = state.bodies.iter().filter(|b| b.body.is_mesh()).map(|b| b.name.as_str()).collect();
            match (selected.as_slice(), meshes.as_slice()) {
                ([one], _) | ([], [one]) => (*one).to_owned(),
                ([], []) => return Err(bad(cmd, "the design has no scan: insert one with Insert Mesh")),
                ([], _) => return Err(bad(cmd, "select the scan (a mesh body) to work on")),
                _ => return Err(bad(cmd, "select one scan (mesh body)")),
            }
        }
    };
    Ok(name)
}

/// Move the scan by `by` as one Move feature; `None` when the move is negligible.
fn move_by(s: &mut Session, mesh: &Mesh, by: &Rigid) -> Result<Option<Value>> {
    if !(by.translation.is_finite() && by.rotation.iter().flatten().all(|x| x.is_finite())) {
        return Err(EngineError::Other("the move came out undefined".into()));
    }
    if by.is_negligible(NEGLIGIBLE, mesh.reach()) {
        return Ok(None);
    }
    let t = by.translation;
    let mut p = json!({"bodies": [mesh.body], "translate": [t.x, t.y, t.z]});
    if let Some((axis, angle)) = by.axis_angle() {
        p["axis"] = json!([axis.x, axis.y, axis.z]);
        p["angle"] = json!(format!("{:.12} deg", angle.to_degrees()));
    }
    let mover = super::find_command("solid.move").ok_or_else(|| EngineError::UnknownCommand("solid.move".into()))?;
    mover.run_nested(s, &p).map(Some)
}

fn base_reply(b: &Base) -> ScanBase {
    ScanBase {
        kind: b.kind.name().into(),
        normal: [b.normal.x, b.normal.y, b.normal.z],
        anchor: [b.anchor.x, b.anchor.y, b.anchor.z],
        area: b.area,
        share: b.share,
        tip_degrees: b.tip.to_degrees(),
        rms: b.rms,
    }
}

fn place_reply(body: String, base: Option<&Base>, feature: Option<Value>) -> Result<Value> {
    let reply = ScanPlaceResult {
        body,
        moved: feature.is_some(),
        base: base.map(base_reply),
        base_index: None,
        base_count: None,
        mirror: None,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn mirror_reply(m: &Mirror) -> ScanMirror {
    ScanMirror {
        point: [m.point.x, m.point.y, m.point.z],
        normal: [m.normal.x, m.normal.y, m.normal.z],
        rms: m.rms,
        coverage: m.coverage,
        symmetric: m.is_symmetric(),
    }
}

/// `guide`, if given: `{from, to, toward?}`.
fn guide(p: &Value, cmd: &str) -> Result<Option<Guide>> {
    let Some(g) = p.get("guide").filter(|v| !v.is_null()) else { return Ok(None) };
    let point = |key: &str, required: bool| -> Result<Option<Vec3>> {
        match g.get(key).filter(|v| !v.is_null()) {
            Some(v) => crate::params::vec3(v).map(Some).ok_or_else(|| bad(cmd, format!("`guide.{key}` must be a point [x, y, z] of numbers"))),
            None if required => Err(bad(cmd, format!("`guide.{key}` is required"))),
            None => Ok(None),
        }
    };
    if !g.is_object() {
        return Err(bad(cmd, "`guide` must be {from, to, toward?}"));
    }
    let (Some(from), Some(to)) = (point("from", true)?, point("to", true)?) else {
        return Err(bad(cmd, "`guide` must be {from, to, toward?}"));
    };
    Ok(Some(Guide { from, to, toward: point("toward", false)?.unwrap_or(Vec3::Z) }))
}

/// A guide from two points picked on the scan: the line between them, seen square to the scan's
/// surface there (the plane through a line drawn along the middle of a part stands up from it).
fn picked_guide(s: &Session, mesh: &Mesh) -> Option<Guide> {
    let picks: Vec<Vec3> = s
        .selection
        .iter()
        .filter_map(|x| match x {
            Sel::Face { body, point, .. } | Sel::Edge { body, point, .. } if *body == mesh.body => Some(*point),
            _ => None,
        })
        .collect();
    let (&from, &to) = (picks.first()?, picks.last()?);
    if from.dist(to) < 1e-6 {
        return None;
    }
    let scan = mesh.scan();
    let normal_at = |p: Vec3| {
        let [a, b, c] = scan.corners(base::nearest_triangle(&scan, p)?)?;
        (b - a).cross(c - a).normalized()
    };
    let (n1, n2) = (normal_at(from)?, normal_at(to)?);
    // Either winding: the two normals are made to agree before they are averaged.
    let toward = (n1 + if n1.dot(n2) < 0.0 { -n2 } else { n2 }).normalized()?;
    Some(Guide { from, to, toward })
}

/// A plane given as `plane: {point, normal}`, if any.
fn plane_param(p: &Value, cmd: &str) -> Result<Option<(Vec3, Vec3)>> {
    let Some(v) = p.get("plane").filter(|v| !v.is_null()) else { return Ok(None) };
    let at =
        |key: &str| v.get(key).and_then(crate::params::vec3).ok_or_else(|| bad(cmd, format!("`plane.{key}` must be a point [x, y, z] of numbers")));
    Ok(Some((at("point")?, at("normal")?)))
}

/// The mirror plane a command works with: the plane given, else the one near the guide drawn (or
/// the two points picked), else the best one found. A plane given or guided to is the one asked
/// for, so it is taken however little of the scan matches it (the reply says how much); the best
/// one found unasked is refused when the scan is no mirror image about it and `refuse` says so.
fn chosen_mirror(s: &Session, mesh: &Mesh, p: &Value, cmd: &str, refuse: bool) -> Result<Mirror> {
    let scan = mesh.scan();
    if let Some((point, normal)) = plane_param(p, cmd)? {
        return symmetry::measure(&scan, point, normal).ok_or_else(|| bad(cmd, "the plane has no direction, or the scan is too small"));
    }
    if let Some(g) = guide(p, cmd)?.or_else(|| picked_guide(s, mesh)) {
        return symmetry::find_guided(&scan, &g).ok_or_else(|| bad(cmd, "the guide marks no plane: draw a line with some length, not end-on"));
    }
    let found = symmetry::find(&scan).ok_or_else(|| bad(cmd, "the scan is too small to look for a mirror plane"))?;
    if refuse && !found.is_symmetric() {
        return Err(bad(
            cmd,
            format!(
                "the scan is not a mirror image about the best plane found: only {:.0} % of it matches its reflection (typically {:.2} mm off); give the plane, or draw a guide along it, to use it anyway",
                100.0 * found.coverage,
                found.rms
            ),
        ));
    }
    Ok(found)
}

fn find_symmetry(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.symmetry";
    // The parameters first, so a malformed call is told so before anything else.
    guide(p, cmd)?;
    plane_param(p, cmd)?;
    let keep = crate::params::bool_(p, "keep").unwrap_or(true);
    let mesh = mesh(s, p, cmd)?;
    let found = chosen_mirror(s, &mesh, p, cmd, true)?;
    let plane = if keep {
        // Three points of the plane, a quarter of the scan apart, make an ordinary construction plane.
        let (u, v) = found.normal.any_perp().normalized().map(|u| (u, found.normal.cross(u))).unwrap_or((Vec3::X, Vec3::Y));
        let reach = (found.size / 4.0).max(1.0);
        let (a, b, c) = (found.point, found.point + u * reach, found.point + v * reach);
        let maker =
            super::find_command("construct.plane.three_points").ok_or_else(|| EngineError::UnknownCommand("construct.plane.three_points".into()))?;
        let at = |q: Vec3| json!({"point": [q.x, q.y, q.z]});
        // Found again (another guide, another scan), the plane is kept beside the last one.
        let taken = |n: &str| s.doc.features.iter().any(|f| f.name == n);
        let name = std::iter::once("Symmetry Plane".to_owned())
            .chain((2..).map(|i| format!("Symmetry Plane {i}")))
            .find(|n| !taken(n))
            .unwrap_or_else(|| "Symmetry Plane".into());
        Some(maker.run_nested(s, &json!({"a": at(a), "b": at(b), "c": at(c), "name": name}))?)
    } else {
        None
    };
    let reply = ScanSymmetryResult { body: mesh.body, mirror: mirror_reply(&found), plane, param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn square_to_symmetry(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.square_to_symmetry";
    // The parameters first, so a malformed call is told so before anything else.
    guide(p, cmd)?;
    plane_param(p, cmd)?;
    let mesh = mesh(s, p, cmd)?;
    let found = chosen_mirror(s, &mesh, p, cmd, true)?;
    let by = symmetry::square_to(&found).ok_or_else(|| bad(cmd, "the mirror plane has no direction"))?;
    let feature = move_by(s, &mesh, &by)?;
    let reply = ScanPlaceResult {
        body: mesh.body,
        moved: feature.is_some(),
        base: None,
        base_index: None,
        base_count: None,
        mirror: Some(mirror_reply(&found)),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn bases(s: &mut Session, p: &Value) -> Result<Value> {
    let mesh = mesh(s, p, "scan.bases")?;
    let found = base::bases(&mesh.scan());
    let reply = ScanBasesResult { body: mesh.body, bases: found.iter().map(base_reply).collect(), param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

fn stand_on_base(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.stand_on_base";
    // The parameters first, so a malformed call is told so before anything else.
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let index = match p.get("base").filter(|v| !v.is_null()) {
        Some(v) => Some(v.as_u64().ok_or_else(|| bad(cmd, "`base` must be a whole number from 0"))?),
        None => None,
    };
    let mesh = mesh(s, p, cmd)?;
    let scan = mesh.scan();
    // A point given or picked on the scan grows the face there; else a base by number, or the
    // next one after the base the scan was last stood on by pressing again.
    let point = point.or_else(|| if index.is_none() { picked_point(s, &mesh.body) } else { None });
    let (chosen, numbered) = match point {
        Some(point) => {
            let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
            (base::grow(&scan, seed).map_err(|e| bad(cmd, e.to_string()))?, None)
        }
        None => {
            let found = base::bases(&scan);
            let count = found.len();
            let i = match index {
                Some(i) => usize::try_from(i).unwrap_or(usize::MAX),
                // The next base while the scan still stands on the last one (nothing has moved it).
                None => match &s.scan_base_cycle {
                    Some((body, last)) if *body == mesh.body && count > 0 && stands_on(found.get(*last), mesh.reach()) => (last + 1) % count,
                    _ => 0,
                },
            };
            let base = found.get(i).copied().ok_or_else(|| bad(cmd, format!("`base` must be below {count}, the number of bases found")))?;
            (base, Some((i, count)))
        }
    };
    let stand = place::stand(chosen.normal, chosen.anchor).ok_or_else(|| bad(cmd, "that base has no direction to stand on"))?;
    let feature = move_by(s, &mesh, &stand)?;
    s.scan_base_cycle = numbered.map(|(i, _)| (mesh.body.clone(), i));
    let reply = ScanPlaceResult {
        body: mesh.body,
        moved: feature.is_some(),
        base: Some(base_reply(&chosen)),
        base_index: numbered.map(|(i, _)| i),
        base_count: numbered.map(|(_, n)| n),
        mirror: None,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The scan's name and the box round its points: where a guide drawn in the view is put (at the
/// depth of its middle) and how big the plane's sheet is drawn.
pub fn scan_box(s: &Session, body: Option<&str>) -> Result<(String, Vec3, Vec3)> {
    let mesh = mesh(s, &json!({ "body": body }), "scan.symmetry")?;
    let (lo, hi) = mesh.scan().bounds().ok_or_else(|| bad("scan.symmetry", "the scan has no points"))?;
    Ok((mesh.body, lo, hi))
}

/// What the Symmetry dialog shows: the mirror plane found (near `guide`, a line from, to and the
/// way it was seen from, when one is drawn) and how well the scan matches itself in it. A scan
/// that is no mirror image is shown as such, not refused.
pub fn symmetry_preview(s: &Session, body: Option<&str>, guide: Option<[Vec3; 3]>) -> Result<ScanMirror> {
    let cmd = "scan.symmetry";
    let mesh = mesh(s, &json!({ "body": body }), cmd)?;
    let scan = mesh.scan();
    let found = match guide {
        Some([from, to, toward]) => symmetry::find_guided(&scan, &Guide { from, to, toward }),
        None => symmetry::find(&scan),
    };
    found.map(|m| mirror_reply(&m)).ok_or_else(|| bad(cmd, "no plane found: draw a guide with some length, not end-on"))
}

/// How far each point of the scan lands from the scan when reflected in the plane through `point`
/// square to `normal`, for the Symmetry dialog's colouring: one value per position of the scan's
/// mesh (NaN for one no triangle uses) and where the colours end.
pub fn mismatch_preview(s: &Session, body: Option<&str>, point: Vec3, normal: Vec3) -> Result<(String, solidcraft_scan::symmetry::Mismatch)> {
    let cmd = "scan.symmetry";
    let mesh = mesh(s, &json!({ "body": body }), cmd)?;
    let plane = symmetry::measure(&mesh.scan(), point, normal).ok_or_else(|| bad(cmd, "the scan is too small to measure against its reflection"))?;
    Ok((mesh.body.clone(), symmetry::mismatch(&mesh.scan(), &plane)))
}

/// The colour the mirror fill is drawn in: amber, an unscanned patch, as a hole fill would be.
const FILL_COLOR: [f32; 3] = [0.95, 0.62, 0.15];

fn complete_by_mirroring(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.complete_by_mirroring";
    // The parameters first, so a malformed call is told so before anything else.
    guide(p, cmd)?;
    plane_param(p, cmd)?;
    let mesh = mesh(s, p, cmd)?;
    let scan = mesh.scan();
    let reach = match crate::params::expr(p, "reach") {
        Some(_) => Some(length(s, p, "reach", 1.0, cmd)?),
        None => None,
    };
    // A scan seen from one side matches little of itself: the plane is used however little.
    let plane = chosen_mirror(s, &mesh, p, cmd, false)?;
    let fill = solidcraft_scan::mirror_fill::complete(&scan, &plane, reach)
        .ok_or_else(|| bad(cmd, "nothing is missing: every reflected triangle lands on the scan (a smaller reach finds smaller gaps)"))?;
    let taken: Vec<String> = s.model.state().bodies.iter().map(|b| b.name.clone()).collect();
    let name = std::iter::once(format!("{} Mirror Fill", mesh.body))
        .chain((2..).map(|i| format!("{} Mirror Fill {i}", mesh.body)))
        .find(|n| !taken.contains(n))
        .unwrap_or_else(|| "Mirror Fill".into());
    let data = solidcraft_doc::MeshData {
        name: name.clone(),
        positions: fill.positions.iter().flat_map(|q| [q.x, q.y, q.z]).collect::<Vec<f64>>().into(),
        triangles: fill.triangles.iter().flatten().copied().collect::<Vec<u32>>().into(),
        color: Some(FILL_COLOR),
    };
    let kind = solidcraft_doc::FeatureKind::MeshImport { file: format!("{} (mirror fill)", mesh.body), meshes: vec![data] };
    let feature = super::features::add_feature(s, &json!({ "body_name": name, "name": "Complete by Mirroring" }), kind)?;
    let reply = ScanCompleteResult {
        body: mesh.body,
        fill: name,
        mirror: mirror_reply(&plane),
        pieces: fill.pieces,
        triangles: fill.triangles.len(),
        area: fill.area,
        joined: fill.joined,
        reach: fill.reach,
        warning: (!plane.is_symmetric()).then(|| unlike(&plane)),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Why what Complete by Mirroring added may not be the part: the scan is no mirror image about the
/// plane. A scan of one side of a mirror-image part matches little of itself too, so it warns
/// rather than refuses.
fn unlike(plane: &Mirror) -> String {
    format!(
        "only {:.0} % of the scan matches its reflection in this plane (typically {:.2} mm off): what was added is right for a scan of one side of a mirror-image part, but a part that is no mirror image does not have it; check it, or undo",
        100.0 * plane.coverage,
        plane.rms
    )
}

/// What the Stand on Base dialog shows before it stands the scan: the base and the scan's points to
/// mark on it (where the part would touch the ground, or the face grown from the pick).
#[derive(Clone, Debug)]
pub struct BasePreview {
    pub body: String,
    pub base: ScanBase,
    /// Which of the scan's bases it is, when it is not a face grown from a pick.
    pub index: Option<usize>,
    /// How many bases the scan has.
    pub count: usize,
    pub marked: Vec<[f32; 3]>,
}

/// The base `scan.stand_on_base` would stand the scan `body` on (the dialog's preview): the face
/// grown from `point` when there is one, else base number `index` (wrapped round the bases).
pub fn base_preview(s: &Session, body: Option<&str>, point: Option<Vec3>, index: usize) -> Result<BasePreview> {
    let cmd = "scan.stand_on_base";
    let mesh = mesh(s, &json!({ "body": body }), cmd)?;
    let scan = mesh.scan();
    let found = base::bases(&scan);
    let count = found.len();
    let (chosen, index, triangles) = match point {
        Some(point) => {
            let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
            let (grown, triangles) = base::grow_face(&scan, seed).map_err(|e| bad(cmd, e.to_string()))?;
            (grown, None, triangles)
        }
        None => {
            let i = if count == 0 { 0 } else { index % count };
            let chosen = *found.get(i).ok_or_else(|| bad(cmd, "the scan has no base to stand on"))?;
            (chosen, Some(i), base::touching(&scan, &chosen))
        }
    };
    // Each point once, however many of its triangles are marked.
    let mut seen = vec![false; mesh.positions.len()];
    let mut marked = Vec::new();
    for &t in &triangles {
        for &v in mesh.triangles.get(t).into_iter().flatten() {
            if let (Some(slot), Some(p)) = (seen.get_mut(v as usize), mesh.positions.get(v as usize))
                && !*slot
            {
                *slot = true;
                marked.push([p.x as f32, p.y as f32, p.z as f32]);
            }
        }
    }
    Ok(BasePreview { body: mesh.body, base: base_reply(&chosen), index, count, marked })
}

/// Whether the scan already stands on `base`: standing it there would not move it.
fn stands_on(base: Option<&Base>, reach: f64) -> bool {
    base.and_then(|b| place::stand(b.normal, b.anchor)).is_some_and(|m| m.is_negligible(NEGLIGIBLE, reach))
}

/// A placing move: `how` gives the move for the scan, or `None` with the reason in `refusal`.
fn place_by(s: &mut Session, p: &Value, cmd: &str, how: fn(&Scan) -> Option<Rigid>, refusal: &str) -> Result<Value> {
    let mesh = mesh(s, p, cmd)?;
    let by = how(&mesh.scan()).ok_or_else(|| bad(cmd, refusal))?;
    let feature = move_by(s, &mesh, &by)?;
    place_reply(mesh.body, None, feature)
}

fn rest_on_grid(s: &mut Session, p: &Value) -> Result<Value> {
    place_by(s, p, "scan.rest_on_grid", place::rest_on_grid, "the scan has no points")
}

fn center(s: &mut Session, p: &Value) -> Result<Value> {
    place_by(s, p, "scan.center", place::center_on_grid, "the scan has no points")
}

fn quick_align(s: &mut Session, p: &Value) -> Result<Value> {
    place_by(s, p, "scan.quick_align", place::quick_align, "the scan is too round to have a longest direction to lay along X")
}

/// A scan's deviation from a model, measured by `scan.deviation`, for the viewport to colour:
/// one value per position of the scan's mesh (see [`solidcraft_scan::deviation::Deviation`]).
/// It stands for the design as it was at `revision`.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviationMap {
    pub scan: String,
    pub body: String,
    pub revision: u64,
    pub values: Vec<f32>,
    pub tolerance: f64,
    pub limit: f64,
    pub rms: f64,
    pub within: f64,
}

impl DeviationMap {
    /// Whether it still stands for the design: nothing has changed since it was measured.
    pub fn current(&self, s: &Session) -> bool {
        self.revision == s.revision
    }

    /// Where a point's value sits on the colour scale, -1 to 1 (0 within the tolerance); `None` for
    /// a point the model leaves out.
    pub fn scale(&self, value: f32) -> Option<f64> {
        solidcraft_scan::deviation::scale(value, self.tolerance, self.limit)
    }
}

/// A length parameter in mm, or `default`; above 0.
fn length(s: &Session, p: &Value, key: &str, default: f64, cmd: &str) -> Result<f64> {
    let Some(e) = crate::params::expr(p, key) else { return Ok(default) };
    let v = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("{key}: {e}")))?;
    if !(v.is_finite() && v > 0.0) {
        return Err(bad(cmd, format!("`{key}` must be a length above 0")));
    }
    Ok(v)
}

fn deviation(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.deviation";
    let reply = |r: ScanDeviationResult| serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()));
    let empty = ScanDeviationResult {
        shown: false,
        scan: None,
        body: None,
        rms: None,
        mean: None,
        max_above: None,
        max_below: None,
        within: None,
        measured: None,
        unmatched: None,
        tolerance: None,
        limit: None,
        param_warnings: Vec::new(),
    };
    if crate::params::bool_(p, "show") == Some(false) {
        s.scan_deviation = None;
        return reply(empty);
    }
    let tolerance = length(s, p, "tolerance", 0.1, cmd)?;
    let limit = length(s, p, "limit", 0.5, cmd)?;
    if !(limit > tolerance) {
        return Err(bad(cmd, "`limit` must be more than `tolerance`"));
    }
    let scan_name = str_(p, "scan").filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let mesh = mesh(s, &json!({ "body": scan_name }), cmd)?;
    let state = s.model.state();
    let model_name = match str_(p, "body").filter(|b| !b.trim().is_empty()) {
        Some(name) => name.to_owned(),
        None => {
            // A solid or surface body; with none in the design, another mesh body: a CAD model
            // that came as an STL.
            let solids: Vec<&str> = state.bodies.iter().filter(|b| !b.body.is_mesh()).map(|b| b.name.as_str()).collect();
            let models: Vec<&str> = if solids.is_empty() {
                state.bodies.iter().filter(|b| b.body.is_mesh() && b.name != mesh.body).map(|b| b.name.as_str()).collect()
            } else {
                solids
            };
            let mut selected: Vec<&str> = s
                .selection
                .iter()
                .filter_map(|x| match x {
                    Sel::Body { name } | Sel::Face { body: name, .. } | Sel::Edge { body: name, .. } => Some(name.as_str()),
                    _ => None,
                })
                .filter(|n| models.contains(n))
                .collect();
            selected.sort_unstable();
            selected.dedup();
            match (selected.as_slice(), models.as_slice()) {
                ([one], _) | ([], [one]) => (*one).to_owned(),
                ([], []) => return Err(bad(cmd, "the design has no model to measure the scan against: build one, or insert the model's mesh")),
                _ => return Err(bad(cmd, "select the model (a solid, surface or mesh body) to measure the scan against")),
            }
        }
    };
    if model_name == mesh.body {
        return Err(bad(cmd, "measure the scan against another body, not itself"));
    }
    let model = state.body(&model_name).ok_or_else(|| bad(cmd, format!("no body `{model_name}`")))?;
    let d = match model.body.display_mesh().filter(|_| model.body.is_mesh()) {
        // A mesh body (the model as an STL) is measured against as it is, its triangles shared.
        Some(triangles) => solidcraft_scan::deviation::measure(&mesh.positions, &triangles, tolerance, 4.0 * limit),
        None => {
            // Meshed within a quarter of the tolerance, as Compare Bodies does, so the mesh agrees
            // with the surface well within what is measured.
            let surface =
                model.body.mesh(&solidcraft_kernel::MeshTolerance::chord(tolerance / 4.0)).map_err(|e| bad(cmd, format!("`{model_name}`: {e}")))?;
            solidcraft_scan::deviation::measure(&mesh.positions, &surface, tolerance, 4.0 * limit)
        }
    };
    if d.measured == 0 {
        return Err(bad(cmd, format!("the scan is nowhere near `{model_name}`: no point is within {:.3} mm of it", 4.0 * limit)));
    }
    s.scan_deviation = Some(std::sync::Arc::new(DeviationMap {
        scan: mesh.body.clone(),
        body: model_name.clone(),
        revision: s.revision,
        values: d.values,
        tolerance,
        limit,
        rms: d.rms,
        within: d.within,
    }));
    reply(ScanDeviationResult {
        shown: true,
        scan: Some(mesh.body),
        body: Some(model_name),
        rms: Some(d.rms),
        mean: Some(d.mean),
        max_above: Some(d.max_above),
        max_below: Some(d.max_below),
        within: Some(d.within),
        measured: Some(d.measured),
        unmatched: Some(d.unmatched),
        tolerance: Some(tolerance),
        limit: Some(limit),
        ..empty
    })
}

fn fit_surface(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fit_surface";
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let controls = match p.get("controls").filter(|v| !v.is_null()) {
        Some(v) => v.as_u64().filter(|n| (4..=30).contains(n)).ok_or_else(|| bad(cmd, "`controls` must be a whole number from 4 to 30"))? as usize,
        None => 8,
    };
    let smoothing = match p.get("smoothing").filter(|v| !v.is_null()) {
        Some(v) => v.as_f64().filter(|x| (0.0..=1.0).contains(x)).ok_or_else(|| bad(cmd, "`smoothing` must be a number from 0 to 1"))?,
        None => 0.1,
    };
    let mesh = mesh(s, p, cmd)?;
    let scan = mesh.scan();
    let radius = length(s, p, "radius", (scan.extent() / 10.0).max(1e-3), cmd)?;
    let point =
        point.or_else(|| picked_point(s, &mesh.body)).ok_or_else(|| bad(cmd, "pick a point on the scan first, in the middle of the area to fit"))?;
    let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let fitted = solidcraft_scan::surface::fit_surface(&scan, seed, radius, controls, smoothing).map_err(|e| bad(cmd, e.to_string()))?;
    // The surface as a sheet, kept as STEP in a Scan Surface feature: the design reopens anywhere.
    let (name, feature) = surface::keep_surface(s, &fitted.surface, "Surface", "Fit Surface", &format!("{} (fitted surface)", mesh.body), cmd)?;
    let reply = ScanFitSurfaceResult {
        body: mesh.body,
        surface: name,
        rms: fitted.rms,
        max: fitted.max,
        points: fitted.points,
        controls,
        folds: fitted.trouble[0],
        pinches: fitted.trouble[1],
        crossings: fitted.trouble[2],
        warning: surface::fold_warning(fitted.trouble),
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// What Fit Surface would make, for its dialog: the surface, how close it comes, and the scan's
/// points it was fitted to, to mark.
#[derive(Clone, Debug)]
pub struct SurfacePreview {
    pub body: String,
    pub surface: solidcraft_geom::bspline::NurbsSurface,
    pub radius: f64,
    pub rms: f64,
    pub max: f64,
    pub points: usize,
    /// Its folds, pinches and crossings (Surface Check).
    pub trouble: [usize; 3],
    pub marked: Vec<[f32; 3]>,
}

/// The default radius of the area Fit Surface fits: a tenth of the scan's size.
pub fn default_surface_radius(s: &Session, body: Option<&str>) -> Result<f64> {
    let mesh = mesh(s, &json!({ "body": body }), "scan.fit_surface")?;
    Ok((mesh.scan().extent() / 10.0).max(1e-3))
}

/// The surface `scan.fit_surface` would fit round `point` on the scan `body`.
pub fn surface_preview(s: &Session, body: Option<&str>, point: Vec3, radius: f64, controls: usize, smoothing: f64) -> Result<SurfacePreview> {
    let cmd = "scan.fit_surface";
    let mesh = mesh(s, &json!({ "body": body }), cmd)?;
    let scan = mesh.scan();
    let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let fitted = solidcraft_scan::surface::fit_surface(&scan, seed, radius, controls.clamp(4, 30), smoothing.clamp(0.0, 1.0))
        .map_err(|e| bad(cmd, e.to_string()))?;
    let mut seen = vec![false; mesh.positions.len()];
    let mut marked = Vec::new();
    for &t in &fitted.region {
        for &v in mesh.triangles.get(t).into_iter().flatten() {
            if let (Some(slot), Some(p)) = (seen.get_mut(v as usize), mesh.positions.get(v as usize))
                && !*slot
            {
                *slot = true;
                marked.push([p.x as f32, p.y as f32, p.z as f32]);
            }
        }
    }
    Ok(SurfacePreview {
        body: mesh.body,
        surface: fitted.surface,
        radius,
        rms: fitted.rms,
        max: fitted.max,
        points: fitted.points,
        trouble: fitted.trouble,
        marked,
    })
}

/// The point picked on the scan `body`: a picked face or edge of it.
fn picked_point(s: &Session, body: &str) -> Option<Vec3> {
    s.selection.iter().rev().find_map(|x| match x {
        Sel::Face { body: b, point, .. } | Sel::Edge { body: b, point, .. } if b == body => Some(*point),
        _ => None,
    })
}

fn fit_plane(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_plane", Kind::Plane)
}

fn fit_cylinder(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_cylinder", Kind::Cylinder)
}

fn fit_sphere(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_sphere", Kind::Sphere)
}

fn fit_cone(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_cone", Kind::Cone)
}

fn fit_torus(s: &mut Session, p: &Value) -> Result<Value> {
    fit_shape(s, p, "scan.fit_torus", Kind::Torus)
}

fn fit_shape(s: &mut Session, p: &Value, cmd: &str, kind: Kind) -> Result<Value> {
    let point = match p.get("point").filter(|v| !v.is_null()) {
        Some(v) => Some(crate::params::vec3(v).ok_or_else(|| bad(cmd, "`point` must be a point [x, y, z] of numbers"))?),
        None => None,
    };
    let tolerance = match crate::params::expr(p, "tolerance") {
        Some(e) => {
            let t = s.doc.eval(&e, solidcraft_doc::expr::Kind::Length).map_err(|e| bad(cmd, format!("tolerance: {e}")))?;
            if !(t.is_finite() && t > 0.0) {
                return Err(bad(cmd, "`tolerance` must be a length above 0"));
            }
            Some(t)
        }
        None => None,
    };
    let keep = crate::params::bool_(p, "keep").unwrap_or(true);
    let mesh = mesh(s, p, cmd)?;
    let point = point.or_else(|| picked_point(s, &mesh.body)).ok_or_else(|| bad(cmd, "pick a point on the scan first, on the face to fit"))?;
    let scan = mesh.scan();
    let seed = base::nearest_triangle(&scan, point).ok_or_else(|| bad(cmd, "the scan has no triangles"))?;
    let fitted = fit::fit(&scan, seed, kind, tolerance).map_err(|e| bad(cmd, e.to_string()))?;
    let feature = if keep { Some(keep_shape(s, &fitted, mesh.scan().extent())?) } else { None };
    let at = |v: Vec3| [v.x, v.y, v.z];
    let (mut plane, mut cylinder, mut sphere, mut cone, mut torus) = (None, None, None, None, None);
    match fitted.shape {
        Shape::Plane { normal, .. } => plane = Some(ScanFittedPlane { point: at(fitted.middle), normal: at(normal) }),
        Shape::Cylinder { axis, radius, .. } => {
            cylinder = Some(ScanFittedCylinder {
                point: at(fitted.middle),
                axis: at(axis),
                radius,
                diameter: 2.0 * radius,
                length: 2.0 * fitted.half_length,
            })
        }
        Shape::Sphere { center, radius } => sphere = Some(ScanFittedSphere { center: at(center), radius, diameter: 2.0 * radius }),
        Shape::Cone { apex, axis, half_angle } => cone = Some(ScanFittedCone { apex: at(apex), axis: at(axis), half_angle: half_angle.to_degrees() }),
        Shape::Torus { center, axis, major, minor } => torus = Some(ScanFittedTorus { center: at(center), axis: at(axis), major, minor }),
    }
    let reply = ScanFitResult {
        body: mesh.body,
        plane,
        cylinder,
        sphere,
        cone,
        torus,
        triangles: fitted.triangles.len(),
        area: fitted.area,
        rms: fitted.rms,
        max: fitted.max,
        feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// Keeps a fitted shape as an ordinary construction feature: a plane through three of its points,
/// an axis through two, or a sphere's centre as a point.
fn keep_shape(s: &mut Session, fitted: &Fitted, extent: f64) -> Result<Value> {
    let at = |q: Vec3| json!({"point": [q.x, q.y, q.z]});
    let run = |s: &mut Session, id: &str, p: Value| -> Result<Value> {
        super::find_command(id).ok_or_else(|| EngineError::UnknownCommand(id.into()))?.run_nested(s, &p)
    };
    let reach = (extent / 8.0).max(1.0);
    match fitted.shape {
        Shape::Plane { normal, .. } => {
            let u = normal.any_perp().normalized().unwrap_or(Vec3::X);
            let v = normal.cross(u);
            let m = fitted.middle;
            run(s, "construct.plane.three_points", json!({"a": at(m), "b": at(m + u * reach), "c": at(m + v * reach)}))
        }
        Shape::Sphere { center, .. } => run(s, "construct.point.vertex", json!({"point": at(center)})),
        Shape::Cylinder { axis, .. } | Shape::Cone { axis, .. } => {
            let half = fitted.half_length.max(reach / 2.0);
            run(s, "construct.axis.two_points", json!({"a": at(fitted.middle - axis * half), "b": at(fitted.middle + axis * half)}))
        }
        Shape::Torus { center, axis, .. } => {
            let half = reach / 2.0;
            run(s, "construct.axis.two_points", json!({"a": at(center - axis * half), "b": at(center + axis * half)}))
        }
    }
}

#[path = "scan_align.rs"]
pub mod align;

#[path = "scan_clean.rs"]
pub mod clean;

#[path = "scan_close.rs"]
pub mod close;

#[path = "scan_curve.rs"]
pub mod curve;

#[path = "scan_fillet.rs"]
pub mod fillet;

#[path = "scan_flatten.rs"]
pub mod flatten;

#[path = "scan_free_form.rs"]
pub mod free_form;

#[path = "scan_mesh.rs"]
pub mod mesh;

#[path = "scan_network_edit.rs"]
pub mod network_edit;

#[path = "scan_patches.rs"]
pub mod patches;

#[path = "scan_reconstruct.rs"]
pub mod reconstruct;

#[path = "scan_section.rs"]
pub mod section;

#[path = "scan_select.rs"]
pub mod select;

#[path = "scan_slice.rs"]
pub mod slice;

#[path = "scan_split.rs"]
pub mod split;
#[path = "scan_surface.rs"]
pub mod surface;

#[path = "scan_template.rs"]
pub mod template;

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
