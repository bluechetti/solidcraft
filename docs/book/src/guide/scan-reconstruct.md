# Reconstruct: shapes and features from a scan

The **Reconstruct** panel of the **Scan** tab reads what the scan is made of (planes, cylinders,
cones, spheres, tori) and turns it into ordinary geometry: a fitted face or axis you sketch from,
or editable features (extrudes, holes, rounds) that rebuild the part.

The clips on this page use scan-like copies of parts modelled in SolidCraft itself, remeshed and
given scanner noise, so their numbers can be checked: a 100 × 60 × 12 mm plate with rounded
corners and four kinds of holes, and a 16 mm bar bent over at the top.

## Find All

<video src="../media/scan-reconstruct/find-all.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Find All on the holes plate: after a moment the plate is coloured by shape, its top and sides as planes, each hole wall a cylinder and each countersink a cone"></video>

**Find All** divides the whole scan into planes, cylinders, spheres, cones, tori and freeform
regions, and colours each its own way. Press it again to take the colours away. It is what
**Rebuild** and **Select on Scan**'s *Shape* tool read from; with *All like it* ticked, a click
on one shape selects every one like it (every hole of that size, every face facing that way). On a scan of millions of triangles it
takes seconds to a minute, worked out beside the window.

## Fit Plane, Fit Cylinder and the other fits

<video src="../media/scan-reconstruct/fit-plane.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Plane: a click on the plate's top face grows the area that is flat, the dialog reads direction 0, 0, 1, an average 0.02 mm off and at worst 0.07 mm, and OK keeps a face trimmed to the area"></video>

<video src="../media/scan-reconstruct/fit-cylinder.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Cylinder: a click on the bar's straight run reads a diameter of 15.99 mm along Z, and OK keeps a trimmed face"></video>

**Fit Plane** (on the panel) and **Fit Cylinder**, **Fit Cone**, **Fit Sphere** and **Fit Torus**
(drop-down) fit one shape where you click. Before OK the dialog draws the shape it fitted over the
scan (a plane's outline and normal, a cylinder's or cone's axis and end rings, a sphere's great
circles, a torus's middle circle) and paints the area it was fitted to. The area grows from the click as far as the scan is
that shape, and stops where it turns into something else, so a bar's straight run stops at its
bend. The dialog says the shape's size and how far the scan is off it.

| Choice in the dialog | Meaning | Example or reason to choose it |
| --- | --- | --- |
| Shape | Plane, cylinder, cone, sphere or torus. | Cone for a countersink or a drafted boss. |
| Area | *Grow from the click*, a round of a given size round it, or what is selected. | A round area on a face broken by holes. |
| Scan tolerance | How far off the scan may be and still count as on the shape (default three times the scan's noise). | Raise it on a coarse scan. |
| Keep | A face trimmed to the area, the shape's plane or axis as construction, or both. | An axis to place a hole on. |

<video src="../media/scan-reconstruct/fit-plane-construction.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Plane with Keep set to Construction: OK keeps a construction plane on the face, to sketch on"></video>

<video src="../media/scan-reconstruct/fit-plane-round.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Plane with Area set to Round the click, 15 mm: only the scan within 15 mm of the click is fitted"></video>

<video src="../media/scan-reconstruct/fit-cylinder-both.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Cylinder with Keep set to Both: the axis and end rings drawn before OK, 15.99 mm across; OK keeps a face and the axis"></video>

<video src="../media/scan-reconstruct/fit-cone.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Cone in a countersink of the plate: the cone's half angle reads 44.95 degrees in the 45 degree countersink"></video>

<video src="../media/scan-reconstruct/fit-sphere.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Sphere on a ball: 40.00 mm across, its great circles drawn"></video>

<video src="../media/scan-reconstruct/fit-torus.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Torus on a ring: radius 30 mm, tube 6 mm"></video>

The ball and the ring were modelled in SolidCraft and saved as a mesh, so their fits are exact.
A real scan is noisier:

<video src="../media/scan-reconstruct/fit-cylinder-mug.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fit Cylinder on a real, noisy scan of a mug from the YCB set: a click on its side reads its diameter and axis, and how far the scan strays from the cylinder"></video>

## Rebuild

<video src="../media/scan-reconstruct/rebuild.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Rebuild on the holes plate lists an extrusion 12.00 mm deep with 8 walls and six holes (tapped, countersunk, counterbored and blind) with their diameters; OK builds them as features"></video>

**Rebuild** reads what the shapes Find All found make together, and lists the steps to build the
part: extrusions with their depth, turned parts, holes with their kind and size (counterbored,
countersunk, tapped, blind), patterns of them, shells, rounds and chamfers. Untick a step to leave
it out. OK builds them in order as editable features, each kept only if it brings the body closer
to the scan. *Then reconstruct what is still missing* goes on to build each piece of the scan the
body still leaves unexplained.

**Reconstruct Selection** (drop-down) reads only the part of the scan selected, such as one boss.

<video src="../media/scan-reconstruct/rest.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Rebuild on the block with Then reconstruct what is still missing ticked: the extrusions and holes built, then what was left"></video>

<video src="../media/scan-reconstruct/selection.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Select on Scan, Region, on the block's hole wall selects the whole wall; Reconstruct Selection builds that hole on its own"></video>



## Radius from Scan

<video src="../media/scan-reconstruct/radius.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Radius from Scan: a tap on the plate's rounded corner reads R5.00 mm, outside, 93 degrees"></video>

**Radius from Scan** (drop-down) reads a round where you tap it: its radius, whether it is an
outside or inside round, and how far it turns. When a model edge runs along it, OK fillets that
edge at the radius read. **Fillet from Scan** does the same for edges you pick on a model built
with sharp edges.

<video src="../media/scan-reconstruct/fillet-from-scan.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Fillet from Scan: the model's sharp corner edge picked; the radius the scan shows there, 5 mm, is read, and OK fillets the edge"></video>



## See also

[Align a scan](scan-align.md) comes before this; [Freeform surfaces](scan-freeform.md) covers the
parts no simple shape fits.
