# Check a model against its scan

The **Check** panel of the **Scan** tab says how faithful a model is to the scan it was made from,
or how faithful a made part is to its CAD: the scan coloured by its distance from the model, the
worst places labelled, a pass or fail against the scan tolerance.

The clips on this page use a scan-like copy of a plate modelled in SolidCraft, checked against a
model built the same way but with its corners left sharp.

## Scan Deviation

<video src="../media/scan-check/deviation.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Scan Deviation colours the plate's scan green where it lies within the tolerance of the model, and labels the extremes: most below, 1.45 mm, at a rounded corner of the scan where the model is sharp"></video>

**Scan Deviation** colours the scan by how far it is from the model: green within the scan
tolerance, warm colours above the model and cool ones below. The corner key says the bands, how
much of the scan is within the tolerance, and whether it passes: 95 % within both ways (the scan on the model and the model on the scan), and nothing but a stray speck more than three tolerances apart, so a hole or a boss missing or made up fails it. The most above and
most below are labelled, and the reading under the pointer follows it. Press it again to take the
colours away. The map follows the model when it changes.

## Pin Deviation

<video src="../media/scan-check/pin.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Pin Deviation: clicks on the deviation map leave labels with the distance at each place"></video>

**Pin Deviation** (drop-down) labels places on the map with their distance from the model: click the
scan where you want a reading. Press it again to stop.

## Align to Model

<video src="../media/scan-check/align-to-model.mp4" autoplay loop muted playsinline controls width="100%" aria-label="The plate's scan lies turned and shifted off its model; Align to Model lays it onto the model, which stays put"></video>

**Align to Model** lays the scan onto a solid or surface body, the CAD it is checked against,
fitted to its surface. The model stays put. Do it before **Scan Deviation** when the scan and the
model were made in different places.

## Scan Triangles

<video src="../media/scan-check/triangles.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Scan Triangles turned on close to the plate: its triangles drawn as lines; pressed again, they go"></video>

**Scan Triangles** (drop-down) draws the triangles of the scans shown as lines over them: how fine
a scan is, and what **Reduce Mesh** or **Remesh** made of it (see
[Clean a scan](scan-clean.md#reduce-mesh)). A scan of more than half a million triangles shows those
of a simplified copy. Press it again to stop.

## Inspection checks

**Add Check** and **Measure Checks** (drop-down) keep an inspection plan: checks picked on the
model and measured on the scan.
<video src="../media/scan-check/hole-table.mp4" autoplay loop muted playsinline controls width="100%" aria-label="Hole Table: every round hole of the model labelled with its number, size and kind"></video>

**Hole Table** numbers every round hole of the shown solids with its position and size.

## See also

[Scan to CAD](scan-to-cad.md) for the whole route.
