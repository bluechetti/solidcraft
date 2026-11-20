# Freeform surfaces from a scan

Where a part is no plane, cylinder or other simple shape (a blade, a handle, a moulded cover), the
**Freeform** panel of the **Scan** tab fits smooth surfaces to the scan. Each is kept as a
surface body with its recipe, so **Edit Surface** reshapes it and **Refit to Scan** fits it again
on a new scan, keeping your edits.

While a Freeform tool is open the scan is coloured by how far it is from the surface: green within
the scan tolerance, warmer or cooler where the scan is above or below it.

The clips on this page use scan-like copies of parts modelled in SolidCraft (a moulded housing,
a bent bar and a twisted blade), a helmet from the Smithsonian's open access collection (CC0) and
a hammer from the YCB object set (CC BY 4.0).

## New Free Form

<video src="../media/scan-freeform/free-form.mp4" autoplay loop muted playsinline controls width="100%" aria-label="New Free Form: four corners clicked on the housing's top; a surface is fitted between them, coloured green where the scan is within tolerance; OK opens Edit Surface on it"></video>

**Free Form** makes a surface over corners you click on the scan, in order round the face.
*Follow the scan* adds points where a rounded part needs them, so the surface does not bow away
from it, and the dialog says how much of the scan is within the tolerance. OK keeps it and opens
**Edit Surface**.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Corners | Four corners, or two at a diagonal's ends: the rectangle lies on the face under the first click, its sides level along the face and up it (on a floor, along X and Y), from any view. | Two for a rectangular panel. Two corners along one side make no rectangle: click opposite corners. |
| Fit to scan | Follow the scan, or lie flat between the corners. | Flat for a wall panel. |
| Stiffness | How firmly the surface keeps its shape rather than following every bump. | Raise it on a noisy scan. |
| Points | The control points each way. | More for a face that curves twice. |

## Wrap Surface

<video src="../media/scan-freeform/wrap.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Wrap Surface: a click on the bar's straight run wraps a net round it, fitted to the scan; OK opens Edit Surface on it"></video>

**Wrap Surface** wraps a surface round a tube-like part: an arm, a handle, a pipe, a limb. Click the
middle of it. A net that goes the whole way round is left open at a seam, which **Merge
Surfaces** closes. *Stiffness* starts at the least that keeps the net from folding over itself;
move it to set it by hand.

<video src="../media/scan-freeform/wrap-hammer.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Wrap Surface on a real hammer scan: a click on the handle wraps a net round it; Details set to Fine adds points, so the net follows the handle more closely"></video>

<video src="../media/scan-freeform/wrap-arc.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Wrap Surface on the bar with Arc set to 180 degrees: the net goes only half way round; Stiffness raised to 40 percent keeps it smooth over the bumps"></video>

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Area | Round the click (within *Reach*), or what is selected. | One limb of a part with several. |
| Details | Low, Medium or Fine: 4, 8 or 14 points round the part. | Fine for a handle with grip shapes. |
| Axis | The part's own fitted axis, or X, Y or Z. | Z for a tube known to stand upright. |
| Arc | How far round the net goes. | 180° for the visible half of a part scanned from one side. |
| Stiffness | How firmly the net keeps its shape rather than following every bump. | Raise it on a noisy scan or if it folds. |

## Fit Patches

<video src="../media/scan-freeform/fit-patches.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Patches: a click on the twisted blade covers it with smooth patches, 97 percent of the scan within tolerance; OK keeps them as one surface body"></video>

**Fit Patches** covers part of the scan with smooth patches, even one that curves round: one patch,
blocks of patches sewn into one body, or a quad following the part's edge. *Detail* chooses how
many control points, automatically until the surface is within half the tolerance.

## Fit Surface

<video src="../media/scan-freeform/fit-surface.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Surface (Freeform drop-down): a click on the twisted blade fits one smooth surface round it, its control points chosen until it is within the tolerance, and OK keeps it trimmed to the area"></video>

**Fit Surface** (drop-down) fits one smooth surface round the click and keeps it trimmed to the
area fitted. Use it for one freeform face; Fit Patches for a larger or wrapping part.
It is for a face that is no plane, cylinder or other simple shape: a dome, a moulded cover, a
blade. It picks as many control points as bring the surface within the *Scan tolerance*.

<video src="../media/scan-freeform/fit-surface-helmet.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Surface on the dome of a helmet scan: a click on the dome, Radius raised to 60 mm, the scan coloured by how far it is from the surface; OK keeps the surface, shown with the scan hidden"></video>

<video src="../media/scan-freeform/fit-surface-untrimmed.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Surface on the blade with Trim to the area unticked: the surface spans the whole rectangle round the area, shown with the scan hidden"></video>

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Area | Round the click (within *Radius*), or what is selected. | What is selected for a face with an odd outline. |
| Radius | How far round the click the area reaches. | Most of a dome. |
| Scan tolerance | How close the surface must come to the scan. | Raise it on a noisy scan. |
| Detail | The control points each way: automatic (as many as the tolerance needs) or a number. | Fewer for a smoother surface. |
| Trim to the area | Keep the surface trimmed to the area's outline, or its whole four-sided span. | Untick to trim it yourself later. |

## Edit, refit and finish

<video src="../media/scan-freeform/edit-surface.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Edit Surface on a Free Form over the housing's top: a point dragged up lifts the surface into a smooth bump round it, blue where it stands above the scan, green where it still lies on it"></video>

<video src="../media/scan-freeform/make-flat.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Make Flat on the Free Form: every point moved onto one plane"></video>

<video src="../media/scan-freeform/slice.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Slice Surface: the Free Form and the YZ plane picked, one side kept; Cap is off, since the cut does not close round an open surface"></video>

- **Edit Surface**: drag one of the surface's points and it lifts off the scan with the pointer,
  the points round it following, less the farther they are, so the surface swells smoothly there
  and stays on the scan farther off. Set *Snap to scan* to put a dragged point back onto the scan,
  and *Other points* to *Stay put* to move it alone. (The preference *Edit Surface drags points
  onto the scan* opens it that way, as it was before.) Double-click a Free Form in the timeline to
  open it.
- **Refit to Scan**: fits a Free Form or Fit Patches surface again on the scan as it is now,
  keeping the points you moved by hand.
- **Make Flat**: puts every point of a Free Form on one plane, for a wall or panel face.
- **Merge Surfaces** and **Close Surface**: join free forms whose sides run along each other, and
  close the openings left, into a solid.
- **Slice Surface**: cuts a surface with a plane, keeps one side and caps the cut. *Cap the cut*
  is on only when the cut closes round the surface; on an open Free Form there is nothing to cap.

## See also

[Reconstruct](scan-reconstruct.md) for the parts simple shapes fit, and the
[Scan to CAD](scan-to-cad.md) overview.
