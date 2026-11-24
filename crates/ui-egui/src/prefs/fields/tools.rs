//! Stored preferences on the Tools page (`PAGE_TOOLS`).
//!
//! Each entry is one field of `Prefs`, in `preference_settings!`'s form:
//! `field: Type = default, PAGE[, accept(range)][ => control(...)];`. Add a new preference
//! to the file of the page whose Defaults resets it, in alphabetical order by field name, so
//! two new preferences rarely land on the same line.

macro_rules! tools_preferences {
    ($next:ident $(, $rest:ident)* ; $($fields:tt)*) => {
        $next! { $($rest),* ; $($fields)*
            /// Explicit assembly Move uses joint handles; off keeps the previous Move panel.
            assembly_move_handles: bool = true, PAGE_TOOLS => check_id("joint_move.use_handles");
            /// Feature windows choose Join, New Body or Cut from what their material touches (`op_auto`,
            /// #6901). Off: they keep the operation they open with (Extrude its own Join or Cut), as before.
            auto_operation: bool = true, PAGE_TOOLS => check_id("prefs.auto_operation");
            /// Work on the model directly in the viewport (pull a face to extrude or cut); off by default.
            direct_interaction: bool = false, PAGE_TOOLS;
            /// Drive Joints shows the moved parts' old place as a ghost (its Show ghost box, #5905).
            drive_joints_ghost: bool = true, PAGE_TOOLS => check_tr("prefs.design_tools.ghost");
            /// In Edit Surface, Ctrl+Shift and a drag smooths the surface under the Smoothing Brush's
            /// circle (no need to turn the brush on), and Ctrl+Shift with the wheel sizes the circle, as
            /// a smoothing brush is commonly used. Off: Ctrl+Shift drags and the wheel with Ctrl+Shift do what
            /// they do elsewhere (the wheel zooms), as before.
            edit_surface_brush_keys: bool = true, PAGE_TOOLS => check_id("prefs.edit_surface_brush_keys");
            /// Edit Surface with points on the surface: a dragged point lifts off the scan, moved in the
            /// plane of the screen, and the points round it follow smoothly. On: a dragged point goes onto
            /// the scan under the pointer and the points round it stay put, as before (a point dragged
            /// up slid along the scan instead of rising).
            edit_surface_drags_onto_scan: bool = false, PAGE_TOOLS => check_id("prefs.edit_surface_drags_onto_scan");
            /// Edit Surface opens with its points on the surface (the surface runs through each) and Snap
            /// set to Under the pointer, as freeform surfaces are commonly edited. Off: it opens on the
            /// control net with snapping off, as before.
            edit_surface_on_surface: bool = true, PAGE_TOOLS => check_id("prefs.edit_surface_on_surface");
            /// Dragging Extrude's distance arrow stops at flat faces parallel to the profile over it and
            /// outlines them (`extrude_face_snap`, #2398). Off: the drag moves in round steps only.
            extrude_arrow_face_stops: bool = true, PAGE_TOOLS => check_id("prefs.extrude_arrow_face_stops");
            /// Extrude: with a profile picked, a face, vertex or parallel plane picked off its plane
            /// switches the extent to To Object (off: picks only add profiles, as before). A click into the
            /// Profiles box makes the next picks profiles again.
            extrude_auto_to_object: bool = true, PAGE_TOOLS => check_tr("ui.extrude_auto_to_object");
            /// The length a new Extrude starts at with `extrude_start` `custom` (any length expression).
            extrude_custom_distance: String = "10 mm".into(), PAGE_TOOLS;
            /// Where a new Extrude starts its distance: `fixed` (the default: 10 in the design's unit, as
            /// before), `last_used` (the last new extrude's distance) or `custom` (`extrude_custom_distance`).
            extrude_start: ExtrudeStart = ExtrudeStart::default(), PAGE_TOOLS;
            /// Extrude turns the view to isometric each time it opens, so the depth can be seen (#9176).
            /// Off (the default): the view stays as it is.
            extrude_view_isometric: bool = false, PAGE_TOOLS => check_id("prefs.extrude_view_isometric");
            /// With `extrude_view_isometric`: when Extrude closes (OK or Cancel) the view goes back to
            /// where it was before it turned (#9176). Off (the default): it stays isometric.
            extrude_view_return: bool = false, PAGE_TOOLS => check_id("prefs.extrude_view_return");
            /// A new Solid Fillet opens at the radius the tool ships with, 2 mm, shown in the design's
            /// unit ("0.07874 in" in an inch design): a radius is bound by the faces beside its edges, and
            /// the number read in the design's unit ("2 in" = 50.8 mm) never fits the part (#12418). On: it
            /// opens at the number 2 read in the design's unit, as before.
            fillet_legacy_radius: bool = false, PAGE_TOOLS => check_id_help("prefs.fillet_legacy_radius", "prefs.fillet_legacy_radius.help");
            /// Flat Template shows the rows most templates need (Outline, the scan, Straight Panel, which
            /// side is cut out) and folds the tuning values (gap, smoothing, rounding, finger gap,
            /// stand-off, scan edge) under More. Off: every row shows, as before.
            flat_template_short: bool = true, PAGE_TOOLS => check_id("prefs.flat_template_short");
            /// While New Free Form, Wrap Surface, Fit Surface or Edit Surface is open, the scan is not
            /// painted in the selection colour, so the surface, its points and the scan's colouring stay
            /// in sight. Off: the scan clicked is painted as selected, as before.
            free_form_plain_scan: bool = true, PAGE_TOOLS => check_id("prefs.free_form_plain_scan");
            /// The holes one Hole command makes (two or more) go into a feature folder in History, closed
            /// at first (`hole_place`). Off by default: each hole is its own entry in History, as before.
            hole_group: bool = false, PAGE_TOOLS => check_id("prefs.design_tools.hole_group");
            /// A placed Single hole has a handle that drags it, and a pick elsewhere on a face that has a
            /// hole moves the hole there (`hole_place`). Off by default: a second pick on its face takes
            /// the hole out, and a hole is moved by taking it out and picking again.
            hole_moves: bool = false, PAGE_TOOLS => check("Move holes by clicking or dragging");
            /// A face takes several Single holes: a pick elsewhere on a face that has a hole adds another,
            /// and a pick on a hole takes it out (`hole_place`). Off by default: a face takes one Single
            /// hole, and a second pick on it takes the hole out (or moves it, with `hole_moves`).
            hole_several: bool = false, PAGE_TOOLS => check_id("prefs.design_tools.hole_several");
            /// Placing a Single hole snaps to the middle of the face and the centres of its round edges
            /// (`hole_place`). Off by default: a hole goes where it is clicked.
            hole_snaps: bool = false, PAGE_TOOLS => check("Snap to face and edge centres");
            /// Degrees per step near the assembly Move ring.
            joint_rotation_coarse: f64 = 10.0, PAGE_TOOLS;
            /// Degrees per step after pulling the cursor outside the precision guide.
            joint_rotation_fine: f64 = 0.1, PAGE_TOOLS;
            /// Show world X/Y/Z component dimensions beside the total distance in Measure.
            measure_axis_distances: bool = true, PAGE_TOOLS;
            /// While Move's triad drags a component drawn as a layer of its own, what is under the pointer
            /// is still picked and highlighted, as before #7273. Off by default: such a drag picks nothing,
            /// so a step neither meshes the dragged bodies where they stand nor looks up the Browser row a
            /// highlight follows (`drag_layer::holds_pointer`).
            move_drag_highlights: bool = false, PAGE_TOOLS => check_id("prefs.move_drag_highlights");
            /// Handles on the preview of a feature placed on a face (Rest, Boss, Snap Fit, and the Box,
            /// Cylinder, Cone, Sphere, Torus and Coil): a marker that drags it across the model and, where
            /// it can turn in the face, a ring that turns it. Off by default: a Rest, Boss or Snap Fit is
            /// then placed by its click only, as before, and its Position input takes faces only; a
            /// primitive by its fields, its inputs or the selection it opened with.
            placement_handles: bool = false, PAGE_TOOLS => check_tr("ui.placement_handles");
            /// Reuse a command's safe values and options after it succeeds, until the app closes. Off by default: each dialog opens with its usual defaults.
            remember_dialog_values: bool = false, PAGE_TOOLS => check_id_help("prefs.remember_dialog_values", "prefs.remember_dialog_values.help");
            /// Remove Floating Pieces, Fill Holes, Reduce Mesh, Repair Mesh, Denoise and Remesh write the
            /// cleaned scan back into its Insert Mesh feature, as before, instead of adding a step to the
            /// timeline that keeps their settings (and can be edited, suppressed or deleted later).
            scan_clean_in_place: bool = false, PAGE_TOOLS => check_id("prefs.scan_clean_in_place");
            /// A click on a scan in a Scan-tab tool (New Free Form, Curve on Scan, Flat Template, Fit
            /// Patches) lands on the nearest corner, edge or crease of the scan, or point of a curve drawn
            /// on it, when one is within a few pixels, and the view shows where. Off: the click lands
            /// exactly where it is, as before.
            scan_click_snap: bool = true, PAGE_TOOLS => check_id("prefs.scan_click_snap");
            /// Scan Deviation's colours reach their end at the distance 99 % of the scan's measured points
            /// lie within, read from the scan, so a few stray points do not wash out the rest. Off: at
            /// 0.5 mm or five tolerances, as before.
            scan_deviation_auto_range: bool = false, PAGE_TOOLS => check_id("prefs.scan_deviation_auto_range");
            /// Scan Deviation's colours in this many steps each side of within (2 to 12); 0, as before, a
            /// smooth scale.
            scan_deviation_bands: f32 = 0.0, PAGE_TOOLS, accept(0.0..=12.0) => slider("prefs.scan_deviation_bands", 0.0..=12.0, 1.0, |v| format!("{v:.0}"), "prefs:scan_deviation_bands");
            /// A scan with no tolerance of its own is read against three times its noise (at least
            /// 0.1 mm) in New Free Form, Edit Surface, Fit Patches and Flat Template, and OK keeps it for
            /// the scan, so a faithful fit to a noisy scan does not read as off. Off: 0.1 mm, as before.
            scan_tolerance_from_noise: bool = true, PAGE_TOOLS => check_id("prefs.scan_tolerance_from_noise");
            /// Section Analysis draws where its plane cuts each shown scan, in the mesh colour, beside the
            /// model's cut. Off: only the model's cut, as before.
            section_scan_outline: bool = true, PAGE_TOOLS => check_id("prefs.section_scan_outline");
            /// Section to Sketch and Sections Along Curve start with a tolerance of three times the cut's
            /// noise (at least 0.1 mm), so a noisy or very large scan is not traced point by point. Off:
            /// they start at 0.1 mm, as before.
            section_tolerance_from_scan: bool = true, PAGE_TOOLS => check_id("prefs.section_tolerance_from_scan");
            /// Snap points (#5912): wherever a point is picked (Move, Align and Measure so far, #5881),
            /// hovering an edge, face or sketch curve shows its centre, middle and end points to snap to. On by default; off, a pick takes only
            /// what is under the pointer, as before.
            snap_points: bool = true, PAGE_TOOLS => check_id("settings.snap_points");
        }
    };
}
