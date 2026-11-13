# Scan to CAD

The **SCAN** tab turns a scan (a mesh or a point cloud made by measuring a real part) into
ordinary parametric geometry: planes and cylinders fitted to it, sketches cut from it, smooth
surfaces drawn over it, and a check of the result against the scan. Its panels run in the order
the work goes: INSERT, CLEAN, ALIGN, FIT, RECONSTRUCT, FREEFORM, SKETCH, SELECT and
CHECK.

A scan stays a mesh body. Everything built from it is a normal feature on the timeline, so
changing the scan's position or a fitted dimension rebuilds what depends on it.

## Big scans

Scans of 50 million to 200 million triangles open and edit. Beyond what the window can draw at
once it shows a simplified copy (its level of detail follows the zoom), while every command reads
the full scan. A few things help:

- A scan file above the import limit is refused with the limit in the message; start SolidCraft
  with `--max-file-size` to raise it, or reduce the scan first with **Reduce Mesh**
  (`scan.reduce`).
- **OK** in a scan dialog computes beside the window, so the window keeps responding.
- On a scan of more than 300 000 triangles the previews of **New Free Form**, **Fit Patches**
  and **Flat Template** are worked out beside the window too: the last preview stays drawn and
  the dialog says *Refining…* until the new one arrives. The preview works from a thinned sample
  of the scan, so it stays quick however big the scan is.
- Commands that must look at every triangle (**Find All**, **Repair**, **Denoise**) take
  seconds to minutes on the biggest scans; use **Select** and **Reconstruct Selection** to work
  on part of one.

## Step by step

**1. Bring the scan in.** **INSERT → Insert Mesh** reads STL, OBJ, PLY, 3MF and glTF; **Point
Clouds** reads clouds. A part that comes in tiny was probably measured in another unit: the import asks, or says
so in a note.

**2. Clean it.** **Repair Mesh** closes cracks and removes faces that pass through each other,
**Fill Holes** closes gaps, **Floating** removes stray pieces, **Denoise** smooths measurement
noise, and **Reduce Mesh** and **Remesh** change the triangle count.

**3. Put it where the part goes.** **Stand on Base** lays a picked face flat on the grid (click
again to cycle through the likely bases), **Center** and **Quick Align** are one click, and
**Symmetry** finds a plane the part mirrors in and lets you drag it and **Square Up** to it.
**Mirror Fill** completes a part that was scanned from one side only.

**4. Fit shapes.** **Fit Plane**, **Fit Cylinder**, **Fit Sphere**, **Fit Cone** and **Fit
Torus** fit a shape to the triangles you click, with the error shown. **Find All** finds them
all, and **Rebuild** (`scan.reconstruct`) proposes the solid features they make together:
**Extrude It**, **Revolve It**, **Cut It**, **Pattern It** and **Fillet It**. Each proposal is
checked against the scan before it is kept.

**5. Draw on it.** **Section to Sketch** (`scan.section_sketch`) cuts the scan with a plane and
sketches the cut as lines and arcs fitted to it. **Curve on Scan** (`scan.curve_on_scan`) lays a
smooth 3D curve along the scan through the points you click, or along a stroke you drag. It is on
the SCAN tab and on the SKETCH tab while a 3D sketch is edited; the curve goes into that sketch,
or into a 3D sketch named `Scan Curves`. **Sections Along Curve** cuts a section at intervals
along a curve.

**6. Make smooth surfaces.** **Fit Surface** fits one B-spline surface to a region. **New Free
Form** makes a surface by hand over the scan: place its corners, then drag the points that lie on
it. **Add Face** grows it across an open edge, **Wrap Surface** wraps a tube round a scan,
**Merge Surfaces** joins two, and **Edit Surface** moves points with falloff, snaps them to the
scan and refits round them. Each of these is kept as a **scan surface** (the Scan Surface
feature in the history), and double-clicking one there opens Edit Surface.

**7. Finish the surfaces.** The FREEFORM panel's drop-down also has **Trim**, **Stitch**, **Patch** and
**Thicken**, the same commands as on the SURFACE tab, so a scan surface can be trimmed to a
curve, stitched to its neighbours into a closed body and thickened without leaving the SCAN tab.

**8. Check it.** **Deviation** colours the scan by its distance from your model.
**Surface Check** finds folds, pinches and crossings in a surface. **Zebra** and **Curvature**
show how smoothly neighbouring surfaces meet and where a surface bends sharply. **Section**
cuts the model, and **Compare** sets it beside another design.

## The same checks and keys in every tool

**One tolerance per scan.** *Scan tolerance* is how far off the scan's surface still counts as on
it. It is kept in the design for that scan (`scan.tolerance`, default 0.1 mm) and set once: the
**Scan tolerance** row of New Free Form, Edit Surface, Fit Patches and Flat Template shows it, and a
change made there is kept when you press OK, so the next tool opens with it. **Deviation** uses it
too.

**One answer to "is this still faithful?"** While you work, those four tools colour the scan under
their result by its distance from it (the same colours and key as **Deviation**) and say one line:
root mean square, mean, the worst distance each way, and the share within the tolerance. Flat
Template colours the wall by how far it stands off the template's plane and counts only the wall
outside the cut-outs, so a flat wall reads as within tolerance and the outlet does not count
against it.

**Clicks snap.** A click on a scan in New Free Form, Curve on Scan, Flat Template and Fit
Patches lands on the nearest corner, edge or crease of the scan, or point of a curve already drawn
on it, when one is within a few pixels, and a marker names it before you click. Hold the snap-suppress
key (Ctrl or Alt by default) to click exactly where the pointer is; **Preferences › Tools ›
Clicks on a scan snap** turns it off. On a scan too big to analyse on the spot it snaps to
curve points only, until a selection tool has read the scan.

**The same keys.** Shift adds to what is chosen and Ctrl or Alt takes away, with a click or a
brush, in Edit Surface and Select. Shift and the wheel size the soft selection; Ctrl+Shift and the
wheel size the brush (Edit Surface's Smoothing Brush and Select's brush).

**Refit after a re-scan.** A surface made by New Free Form or by Fit Patches (from a click) keeps
its recipe in the design: the scan, the command with its parameters, and where the fit put each
control point. **Refit to Scan** (`scan.refit`) fits it again on the scan as it is now (a new scan,
or one moved or cleaned earlier in the timeline) and moves each point you dragged by hand the same
way on the new fit, so a re-scan does not cost your edits. A surface reshaped so its net changed
(a face or a row added) asks for `discard_edits`.

## Selecting part of a scan

**Select** picks triangles with a brush, a polygon or a smooth region; **Inside Curve** picks
those inside a curve on the scan. **Keep Selection** and **Delete Selection** cut the scan down, and **Reconstruct
Selection** and **Curve from Selection** work on the selected part only.

## Survey Scan

**Survey Scan** (SCAN › INSERT menu) takes a first look at a scan without changing it: its size
along its own directions, its noise, holes, floating pieces and edges to repair, and, once Find
All has read it, how much of it is simple shapes. Below that it lists the next steps in the order
the work goes, each with its reason and a button that opens that tool. Open it again after
cleaning and after Find All: the route it suggests follows what they found.

## With an agent

An agent working through SolidCraft's [MCP server](mcp.md) reads the bundled skill
`solidcraft-scan-to-cad` and starts on a scan with **Survey Scan** (`scan.survey`). Survey Scan
changes nothing. It reports the scan's size along its own directions, its noise, its holes,
floating pieces and open edges, whether it is a wall or a panel, and, once Find All has run, how
much of it is planes, cylinders and other shapes. Its `next` field lists the commands to run next
in the order the work goes, each with the reason and parameters to start from. For example, it
leaves a wall's holes open, because on a wall they are the frame's own holes, and it suggests
Denoise only when the noise is a real share of the scan's tolerance.

## Terms

The terms used here (*scan*, *scan surface*, *curve on scan*, *deviation*, *section*, *Free
Form*) are in the [glossary](../reference/glossary.md).
