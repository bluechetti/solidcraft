# Clean a scan

A scan straight from the scanner has holes where the light did not reach, specks of dust and
fixture floating beside the part, triangles that cross or double up, and noise on every surface.
The **Clean** panel of the **Scan** tab fixes these before you fit or sketch anything, because
each of them pulls a fit off the part.

Every tool here leaves the scan you inserted alone: it adds a **Scan Clean** step to the timeline
that keeps only its settings, so you can edit, suppress or delete it later. (Preferences ›
Tools › *Clean scans in place* writes the result back into the scan instead.)

The clips on this page use openly licensed scans: the YCB mug (CC BY 4.0, Calli et al.), the
Stanford Bunny (Stanford Computer Graphics Laboratory) and T-LESS object 5 (CC BY 4.0, Hodan et
al.). Each clip loops; hover it for its controls.

## Survey Scan: start here

<video src="../media/scan-clean/survey.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Survey Scan on a mug scan: its size, noise, 363 holes, 4 floating pieces and 872 edges to repair, then the list of next steps"></video>

**Survey Scan** (Insert panel) looks the scan over without changing it: its size along its own
directions, its noise, how many holes, floating pieces and edges to repair it has. Below that it
lists the next steps in the order the work goes, each with its reason and a button that opens
the tool. Run it again after cleaning: the route follows what is left.

## Clean Up: the usual steps in one go

<video src="../media/scan-clean/clean-up.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Clean Up on the mug: the dialog with Repair Mesh, Remove Floating Pieces and Fill Holes ticked, then OK: the dark slits of the holes are gone and three steps are on the timeline"></video>

**Clean Up** runs **Repair Mesh**, **Remove Floating Pieces** and **Fill Holes** (and, when ticked,
**Reduce Mesh** to a number of triangles you give) one after another. Each is kept as its own
step on the timeline, and one with nothing to do is left out, so a step you don't want can be
deleted on its own.

- **Do** start a new scan here, then look at what is left with Survey Scan.
- **Don't** use it on a scan of a wall or a panel whose holes are real: Fill Holes would close
  them. Untick Fill Holes, or use Fill Holes on its own and untick the real ones.

<video src="../media/scan-clean/clean-up-reduce.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Clean Up with Reduce Mesh ticked: the mug cleaned, then reduced to half its triangles"></video>

Tick **Reduce Mesh** to end with fewer triangles: *Reduce to* is the share kept.

## Repair Mesh

<video src="../media/scan-clean/repair.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Repair Mesh opened from the Clean panel's drop-down on the mug: OK joins its points a hair apart and the black cracks over it close"></video>

**Repair Mesh** (Clean drop-down) fixes the triangles themselves. It joins points a hair apart
(a file that wrote one point twice with its last digits different leaves a crack between them,
drawn as a dark slit), and takes off duplicates, edges shared by three or more triangles, slivers
too thin to use, and triangles passing through others (*Fix faces through each other*). Use it
when Survey Scan counts edges to repair, or when a fit, Fill Holes or Remesh refuses the scan.

## Remove Floating Pieces

<video src="../media/scan-clean/floating.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Remove Floating Pieces on the mug: the small pieces apart from it are painted red, then OK removes them"></video>

**Floating** finds the pieces of the scan not joined to the part (dust, a turntable, a clamp,
a reflection) and paints the ones that will go red.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Floating below | A piece smaller than this share of the scan's area goes. | Raise it to take off a fixture that is a large piece. |
| A click on the scan | Keeps a red piece, or takes off one that would stay. | A loose part scanned beside the first that you want too. |

## Fill Holes

<video src="../media/scan-clean/fill-holes.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fill Holes on the bunny seen from below: the holes in its base outlined and ticked, then OK closes them with smooth patches"></video>

**Fill Holes** outlines every hole and ticks the ones it will close. The scan's outer rim (the
edge of a scan that is open on purpose, such as a wall) is drawn dashed and left open.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Largest hole | Holes wider than this, in mm, are left open. | Keep a real 10 mm bore open: set 8 mm. |
| Fair the patches | The patches follow the curve of the scan round them instead of lying flat. | On by default; off for a hole in a flat face. |
| A click by an outline | Ticks or unticks that hole. A ticked dashed rim is capped flat. | Leave a mounting hole open while the scan gaps close. |

- **Don't** fill a hole across a sharp edge and then fit the edge: the patch is smooth, so the edge
  comes out rounded there.

## Denoise

<video src="../media/scan-clean/denoise.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Denoise on a T-LESS part: the dialog studies each strength, then OK smooths the scan's ripples"></video>

**Denoise** (Clean drop-down) smooths the scanner's noise. It first tries each strength of the
filter and plots how rough the scan stays against how far it moves, then suggests the strength
where the error is lowest (*Use Suggested*). Sharp edges and the open rim stay where they are.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Area | Whole scan, the piece clicked, an area grown from a click, a round in mm round it, or what is selected. | Smooth only the rough side of a part. |
| Strength | Passes of the filter. | Leave it on the suggestion. |

- **Do** check the *Noise* line: on a clean scan the dialog says smoothing would only move it.

## Reduce Mesh

<video src="../media/scan-clean/reduce.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Scan Triangles turned on, then Reduce Mesh on a T-LESS part: the dialog counts 42,901 triangles and about 21,451 kept, and OK leaves a coarser mesh"></video>

**Reduce Mesh** (Clean drop-down) takes triangles away where they do least harm: flat areas go
first. Turn on **Scan Triangles** (Check panel) to see the triangles before and after.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Area | As for Denoise. | Thin out a large flat floor and keep the detail elsewhere. |
| Keep | How many triangles to keep. | Enough to save and fit quickly. |
| Error limit | Stops before the surface would move farther than this. | Half the scan tolerance keeps fits honest. |
| Keep the rim | The open edge of the scan keeps its shape. | On for a wall or a panel. |

## Remesh

<video src="../media/scan-clean/remesh.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Scan Triangles turned on, then Remesh on a T-LESS part: OK replaces the uneven triangles with even ones"></video>

**Remesh** (Clean drop-down; turn on **Scan Triangles** to see it) makes the scan again as one even surface of triangles about the
cell size you ask for, averaging the noise out. Passes already aligned to the scan can be fused
in with it. *Sharp* keeps edges and corners sharp. With an area chosen, only that part is remade
and its outline and creases stay in place.

## Select on Scan

<video src="../media/scan-clean/select.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Select on Scan on the bunny: a brush stroke over its back selects the triangles under it, then Grow widens the selection twice"></video>

**Select** chooses part of a scan for the other tools. Every Clean, Fit and Freeform tool with an
*Area* row can work on *What is selected*.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Tool | Brush (drag over the scan), Polygon or Lasso (round what you see), Region (the smooth area up to creases sharper than the Crease angle: a whole face, a whole hole wall), Shape (a shape Find All found; with All like it, every shape like it), Inside Curve (inside a closed curve drawn on the scan). | Region for one face of a machined part; Shape with All like it for every hole of one size. |
| Selection | Replace, Add or Remove. Shift adds and Ctrl takes away with any tool. | |
| All, None, Invert, Grow, Shrink | Change the selection as a whole. | Grow once to take a fillet round a face. |
| Keep Selection | Keeps it in the design by its name, to open again later. | |

### Each way to select

<video src="../media/scan-clean/select-polygon.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Select on Scan with Tool set to Polygon: four corners clicked round the bunny's head, the first clicked again, and the head on the side seen is selected"></video>

<video src="../media/scan-clean/select-region.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Select on Scan with Tool set to Region on the block: a click on its front face selects that face up to its edges; Grow twice widens it by two rings of triangles"></video>

<video src="../media/scan-clean/select-add-remove.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Select on Scan with the brush: one stroke with Selection set to Replace, a second added with Add, then a stroke across both with Remove takes a band out"></video>

- **Polygon**: click corners round what you see; click the first again (or *Select Inside
  Polygon*) to select. Only the side facing you is taken unless *Through the scan* is ticked.
- **Lasso**: drag round what you see.
- **Region**: click once; the area grows up to creases sharper than the *Crease angle*. On a
  part rounded all over (no crease that sharp) it takes the whole part: select with the brush or
  a polygon there instead.
- **Selection** *Add* and *Remove* build a selection from several strokes or shapes.
- **Shape** with **All like it**: after Find All, a click on one shape selects every shape like
  it: holes of the same diameter, cones of the same angle (countersinks), balls of the same size,
  faces facing the same way.

<video src="../media/scan-clean/select-like.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Find All on the holes plate, then Select on Scan with Tool Shape and All like it ticked: a click on one counterbore's wall selects both counterbores, 452 square mm"></video>

## Split Mesh

<video src="../media/scan-clean/split.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Split Mesh on the bunny: the origin's YZ plane picked in the browser as the cutter, both sides painted, then OK"></video>

**Split Mesh** (Clean drop-down) cuts the scan along a plane, a face, a body, a curve or a sketch
you pick, or a shape fitted where you click the scan. With nothing picked, the selection is split
off. Both sides are painted before you press OK.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Cutter | What cuts. | An origin plane through a symmetric part. |
| Keep | Both sides as two scans, one side, or only the crossing curves. | Crossing Only for a section to sketch from. |
| Envelope | Also keeps a slab this thick round the cutter as a scan of its own. | A band through a part to read its cross-section. |
| Facing Faces Only | Cuts only the side of the scan facing you. | A cut through one wall of a box. |

<video src="../media/scan-clean/split-keep.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Split Mesh along the YZ plane with Keep set to In Front: the side behind the plane is taken off the bunny"></video>

<video src="../media/scan-clean/split-envelope.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Split Mesh along the YZ plane with Envelope set to 10 mm: a band 10 mm thick through the bunny becomes a scan of its own, shown with the rest hidden"></video>



## Complete by Mirroring

<video src="../media/scan-clean/mirror.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Complete by Mirroring on a T-LESS part with one end cut away: the plane across the missing end is found, the amber shows the fill before OK, and OK adds it"></video>

Before OK the dialog draws what OK will add, in amber, and says how many triangles. **Complete by Mirroring** (Clean drop-down) finds the plane the scan mirrors in and adds the
scan's reflection where the scan has nothing, as a body of its own. It is a guess that the two
sides are alike: the dialog says how much of the scan matches its mirror image. When the part
mirrors in more than one plane, or a large part of one side is missing, the plane it finds may
not be the one you mean. Click the plane you mean (an origin plane, or a construction plane such
as the one Find Symmetry keeps) and it is used as it is; or tick *Draw a guide* and drag a line
along the mirror plane, seen square to it, and the plane is looked for near it.

On a part with one end never scanned (its open edge lies near a plane), the plane it looks for
is the one across that end, where the scan's reflection lands back on itself best; the part may
mirror better in another plane (front to back), but that one adds only stray patches. Look at the
amber before OK: it should sit where the scan is missing; if not, pick the plane you mean.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Plane | An origin or construction plane picked: the mirror plane, as it is. | YZ for a part modelled about the origin. |
| Color by mismatch | Colours the scan by how far each point's reflection lands from it. | Green where the two sides agree. |
| Draw a guide | The next drag in the view marks where the plane runs. | When no plane is drawn there yet. |
| On OK | Keep the plane, make it X = 0, or complete by mirroring. | |

## Mesh from Points

<video src="../media/scan-clean/points.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Mesh from Points on one scanner pass of the Stanford bunny: the dots become a smooth mesh body, open where the scanner never saw"></video>

**Mesh from Points** (Clean drop-down) makes a mesh body from a point cloud (inserted with
**Insert › Point Cloud**). Where the points thin out the surface stops, instead of closing over
what was never scanned.

## Remove Markers

<video src="../media/scan-clean/remove-markers.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Remove Markers on a plate with four 6 mm marker balls: one clicked, four found, and OK takes them off and closes the holes"></video>

**Remove Markers** (Clean drop-down) takes reference markers off a scan: click one and every
sphere of its size is found, the dialog says how many, and OK removes them and closes their holes.
A marker on the scan's edge, whose hole would open into the edge, is pressed down onto the surface
round it instead, so no bite is left. On a coarse scan (triangles a third of a marker across) the
markers are still found, but with fewer than about six points across, a marker reads as a bump,
not a ball. The clip's plate and markers were modelled in SolidCraft and saved as a mesh.

## See also

[Scan to CAD](scan-to-cad.md) for the whole route, and the
[command reference](../reference/commands.md) for each command's parameters.
