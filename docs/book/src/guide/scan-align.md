# Align a scan

A scan comes in where the scanner saw it: at an angle, off the origin, often in several passes.
The **Align** panel of the **Scan** tab puts it where the part goes, so that sketches, fitted
planes and the features you build on it line up with the design's own X, Y and Z. Each tool adds
a Move of the scan to the timeline, and whatever you made from the scan moves with it.

The clips on this page use T-LESS object 5 (CC BY 4.0, Hodan et al.), turned and lifted as a scan
arrives, a French Army canteen from the Smithsonian's open access collection (CC0), a scan-like
block modelled in SolidCraft, and two raw passes of the Stanford Bunny (Stanford Computer
Graphics Laboratory).

## Stand on Base

<video src="../media/scan-align/stand-on-base.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Stand on Base on a part lying at an angle: OK stands it on its steadiest base on the grid, and pressing again turns it onto the next base"></video>

**Stand on Base** stands the scan face down on the grid. The dialog marks the base it will use:
the steadiest one first, the next ones in the **Base** list. Press it again while the scan still
stands where it was put and it opens on the next base, so a few presses go round the ways the
part can sit.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Face | A click on the scan grows the flat face there and stands on it; three or more clicks stand it on the plane through them. | The machined underside of a casting. |
| Base | The bases found, steadiest first, while nothing is clicked. | Base 2 when the steadiest is the top. |

Or click the face to stand on:

<video src="../media/scan-align/stand-on-face.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Stand on Base: a click on the block's front face marks the flat face grown from it, and OK tips the block onto that face"></video>

## Align: Quick Align and the other ways

<video src="../media/scan-align/quick-align.mp4" autoplay loop muted playsinline controls width="100%" aria-label="The Align dialog lists the ways to lay a scan; Quick Align turns the tilted part square to X, Y and Z about the origin"></video>

<video src="../media/scan-align/quick-align-canteen.mp4" autoplay loop muted playsinline controls width="100%" aria-label="A scan of a flat canteen lying at an angle: Quick Align lays it square, longest along X, its flat side down on the grid; seen from the front"></video>

**Align** opens a short list of the ways to lay a scan, each with what it is for:

- **Quick Align**: square to its own faces, longest along X and thinnest along Z, centred over
  the origin and resting on the grid. One click for a part that arrived at an angle.
- **Align to Scan**: one scan onto another (below).
- **Align All Scans**: several passes of one part at once, the error shared out.
- **Align by Points**: three or more points clicked on the scan and their partners on another
  scan or model, where Align to Scan fails.
- **Align to Model**: the scan onto the CAD it is checked against (Check panel).

## Align to Scan

<video src="../media/scan-align/align-to-scan.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Two raw passes of the Stanford bunny, grey and orange, lying apart: Align to Scan lays the orange pass onto the grey one and the two interleave as one bunny"></video>

**Align to Scan** lays the selected scan onto another from wherever it lies, as a second pass onto
the first. Pick the pass to move (**Body**) and the one it goes onto (**Target**). The passes
need to overlap; where they don't, use **Align by Points**. **Combine Scans** goes on to fuse
aligned passes into one even scan.

## Align All Scans and Combine Scans

<video src="../media/scan-align/align-all.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Three raw passes of the bunny in grey, orange and blue: Align All Scans lays them all together at once, the first staying put"></video>

<video src="../media/scan-align/combine.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Combine Scans on the three passes: they are aligned and fused into one even scan, and the passes are hidden"></video>

**Align All Scans** (in Align) lays several passes of one part together at once, fitting every
overlapping pair so the error is shared out rather than piling up on the last pass; the first pass
stays put. **Combine Scans** (Align drop-down) goes on to fuse them into one even scan, as
Remesh does, and hides the passes it fused (show them again from the browser).

## Rest on Grid and Center on Origin

<video src="../media/scan-align/rest-and-center.mp4" autoplay loop muted playsinline controls width="100%" aria-label="The part, upright but lifted and off to one side: Rest on Grid brings it down onto the grid, then Center on Origin slides it over the origin"></video>

Two one-click moves in the Align drop-down: **Rest on Grid** moves the scan up or down until its
lowest point is on the grid; **Center on Origin** slides it until the middle of its box is over the
origin, keeping its height.

## Find Symmetry and Square to Symmetry

<video src="../media/scan-align/symmetry.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Find Symmetry draws the mirror plane over the tilted part; with On OK set to Make it X = 0, OK turns the part so the plane is the YZ plane, seen from the top"></video>

**Find Symmetry** (Align drop-down) finds the plane the scan mirrors in and draws it over the scan,
with how much of the scan matches its reflection. **On OK** keeps it as a construction plane,
makes it X = 0 (**Square to Symmetry**: the part is turned so you can model one half about the
origin and mirror it), or completes the scan by mirroring. A part that mirrors in more than one
plane: click the plane you mean, or draw a guide along it.

<video src="../media/scan-align/symmetry-keep.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Find Symmetry: the plane found and 90 percent matched; Color by mismatch paints the scan green where its two sides agree; OK keeps the plane"></video>

<video src="../media/scan-align/symmetry-guide.mp4" autoplay loop muted playsinline controls width="100%" aria-label="A part with two mirror planes: from above, a guide drawn along the other plane finds that one"></video>

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Plane | An origin or construction plane picked: used as it is, not searched for. | YZ for a part modelled about the origin. |
| Color by mismatch | The scan coloured by how far each point's reflection lands from it. | Green where the sides agree, red where they differ. |
| Draw a guide | The next drag in the view marks where the plane runs, seen square to it. | When the part has more than one mirror plane. |
| On OK | Keep the plane, make it X = 0, or complete by mirroring. | Make it X = 0 to model half the part. |

## See also

[Clean a scan](scan-clean.md) comes before this, and [Scan to CAD](scan-to-cad.md) has the whole
route.
