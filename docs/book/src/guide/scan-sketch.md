# Sketch from a scan

The **Sketch** panel of the **Scan** tab draws from the scan: sections fitted as lines and arcs to
extrude, curves laid on the scan for sweeps and edges, patterns unrolled flat, and flat templates
of walls to cut from sheet.

The clips on this page use scan-like copies of parts modelled in SolidCraft (a machined block, a
twisted blade, a moulded housing), a helmet from the Smithsonian's open access collection (CC0),
the Stanford Bunny and a scan of a van's side wall.

## Cut to Sketch

<video src="../media/scan-sketch/section.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Cut to Sketch on the block: the XY plane at mid-height cuts it, the cut drawn as one outline of lines and arcs round the slot and the hole; OK makes the sketch"></video>

**Cut to Sketch** cuts the scan with a plane and sketches the cut as fitted lines, arcs and
circles: the start of a part to dimension and extrude. It opens on the XY plane through the middle
of the scan.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Plane | XY, XZ or YZ (with a sketch open, its own plane). | XZ for a side profile. |
| Offset | How far along the plane's normal; the middle of the scan until you type one. | The height of a boss. |
| Tolerance | How far a piece may stray from the cut: smaller follows it more closely with more pieces. | Raise it on a noisy scan. |

**Sections Along Curve** (drop-down) cuts sections square to a 3D curve at even steps, ready to
loft.

<video src="../media/scan-sketch/sections-along-curve.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Sections Along Curve on the blade: eight sections square to a curve drawn up it, each sketched as lines and arcs"></video>



## Curve on Scan

<video src="../media/scan-sketch/curve.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Curve on Scan: four points clicked up the blade; a smooth curve lies on the scan through them, 69.47 mm long and within 0.05 mm of it; OK keeps it in a 3D sketch"></video>

**Curve on Scan** lays a smooth 3D curve on the scan through points you click, or along a stroke
you drag: a feature line, an edge, a sweep's path. It goes into the open 3D sketch, or into one
named *Scan Curves*. Click the first point again to close it.


<video src="../media/scan-sketch/curve-closed.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Curve on Scan on a helmet: six points clicked round its dome, the first clicked again to close the loop; OK keeps it, shown with the scan hidden"></video>

<video src="../media/scan-sketch/curve-drawn.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Curve on Scan drawn as a stroke: a drag along the bunny's side becomes a smooth curve on the scan, its length and how closely it fits shown"></video>

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Points | The points clicked; *Remove Last Point* takes one back. | |
| Closed | Joins the last point to the first. Clicking the first point again ticks it. | An outline to split along or select inside. |
| Within | How far the smooth curve may stray from the line laid on the scan: smaller follows it more closely with more points. | Smaller for a sharp feature line. |

A closed curve is the cutter for **Split Mesh** (*Curve*) and the outline for **Select on Scan**
(*Inside Curve*).

**Curve from Selection Edge** (drop-down) traces the outline of what is selected instead.

<video src="../media/scan-sketch/curve-from-selection.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Select on Scan, Region, takes the block's top face up to its edges; Curve from Selection Edge traces its outline as a 3D curve"></video>



## Flatten and Roll onto Scan

<video src="../media/scan-sketch/flatten.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Flatten on the housing's rounded corner, 10 mm round the click: it is laid flat with next to no stretch, and its outline is sketched beside the part"></video>

**Flatten** (drop-down) unrolls part of the scan flat, keeping its lengths and angles as well as it
can, sketches its outline, and says how much the pattern stretches. A surface that curves only one
way (a cylinder, a cone, a bent sheet) lies flat with next to no stretch; one that curves both ways
(a dome, a rounded corner of two rounds) cannot, and the dialog says so. Draw on the pattern, then
**Roll onto Scan** carries what you drew back onto the scan as 3D curves.

## Flat Template

<video src="../media/scan-sketch/flat-template.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Flat Template on a van's side wall: Outline set to Whole Scan follows the wall's edge, the wall is coloured by how far it stands off the template, and OK makes the template sketch, shown with the scan hidden"></video>

**Flat Template** makes a flat template of a wall: the outline laid on the wall's fitted plane,
with a cut-out for each part standing off it. *Picked Points* takes the corners you click;
*Whole Scan* follows the wall's own edge and closes small gaps. **Straight Panel** goes further: a
panel cut from sheets of a given size, its seams on the wall's rows of holes, reviewed item by item,
and exported as drawings with **Export Panel**.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Outline | Picked Points or Whole Scan. | Whole Scan for a van wall. |
| Bridge Tears Under | Gaps in the scan narrower than this are closed. | Raise it on a patchy scan. |
| Smooth Away Under | Holes in the wall smaller than this are covered, not cut. | Cable runs and small holes. |
| Round Corners To | Corners and lengths are rounded to this step. | 5 mm for a plywood template. |
| Cut Out | What stands off the wall becomes a cut-out: on both sides, or only what stands out. | |
| Stand-off | How far off the wall counts as standing off. | Automatic. |

The wall is coloured over the stand-off: green where the template covers it, warm where it stands
out (a cut-out), cool where it lies deeper (a bay or pocket).

## See also

[Freeform surfaces](scan-freeform.md) and the [Scan to CAD](scan-to-cad.md) overview.
