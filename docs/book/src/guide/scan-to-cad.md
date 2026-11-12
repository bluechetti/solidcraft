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

## Selecting part of a scan

**Select** picks triangles with a brush, a polygon or a smooth region; **Inside Curve** picks
those inside a curve on the scan. **Keep Selection** and **Delete Selection** cut the scan down, and **Reconstruct
Selection** and **Curve from Selection** work on the selected part only.

## Terms

The terms used here (*scan*, *scan surface*, *curve on scan*, *deviation*, *section*, *Free
Form*) are in the [glossary](../reference/glossary.md).
