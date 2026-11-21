# SolidCraft iPad: AR view

The design in the room through the iPad's camera. Branch `ipad-ar`.

## Using it

Tap **AR** (bottom right of the design view). Apple's coaching overlay asks you to move the iPad
until it finds a surface.

| Input | What it does |
|---|---|
| Tap a surface | Puts the model there, upright, at real size (shrunk to fit if it is over 1.5 m). Tapping again moves it there. |
| One finger drag | Slides the model along the surfaces under the finger. |
| Pinch | Scales it about its foot; it snaps to 100% with a tick. |
| Twist (two fingers) | Turns it about the vertical. |
| Pencil tap | Selects the part under the tip (`select.set`), or clears the selection over nothing. |
| Done / Real size / Pick up | Leave AR / back to 100% / take the model off the surface. |
| Section | A cut through the model facing you; drag up and down to move it; Apply keeps it in the design. |
| Explode | The design's exploded views in turn, then assembled again. |
| Measure | Two Pencil taps on the model: the distance in mm, drawn in the room. |
| Pull | Press the Pencil on a face and drag: an arrow and the distance follow (a tick on Pencil Pro every 5 mm). Lift, then type or write (Scribble) the exact value, Apply (`solid.press_pull`, one undo step) or Cancel; the Pencil back on the arrow adjusts it. On an edge it rounds the edge instead (a fillet, the drag is the radius). |
| Undo / Redo | The design's own undo history. |
| Photo | Saves the room and the model (without the panel) to Photos. |
| Sizes | The model's width, depth and height drawn along its sides, in mm (as it is now); with a part selected, that part's. |
| Scale | 2:1, 1:1, 1:2, 1:5, 1:10, 1:20 in one tap, about the model's foot; the panel names the scale. |
| Move | Press the Pencil on a part and drag it across the level it was grabbed at, in whole millimetres (kept on one axis when nearly along it). Lift, then Apply (`solid.move`, one undo step) or Cancel. |
| Hole | Press the Pencil on a face: the hole's middle follows it, with its distances to the face's nearer edges (whole millimetres, or the face's middle). Lift: a row under the panel sets X / Y (typed or written), Simple / Counterbore / Countersink, Through or a depth, and the size (M2–M10 clearance, M2–M10 tapped, or a diameter). Apply drills it held to those two edges (`solid.hole` references, so it follows them), one undo step. The Pencil on it moves it again. |
| Ink | Draw on the model with the Pencil; the ink stays on its surfaces in the room (and in videos). Clear ink removes it; leaving AR does too. |
| Spin | The model turns slowly about its foot, as on a turntable. |
| Tap a wall | Hangs the model upright with its back against the wall; it is dragged along the wall and has no floor shadow. |
| Record | A screen video of the session (ReplayKit); Stop shows Apple's preview to save or share it. |
| Joints | Each joint's coordinates with − and + (`joint.drive`, held to its limits). |
| Check | Finds overlapping parts (`inspect.interference`), selects them and marks each overlap in the room. |
| Pencil hover | The face under the hovering tip lights up, as under the mouse. |
| Pencil double tap | Undo. |
| Pencil squeeze (Pro) | A tool palette at the tip: Select, Pull, Hole, Ink, Measure, Section, Photo, Undo. |

Snaps, placing and applying play a haptic tick on Apple Pencil Pro (an iPad has no haptic engine
of its own).

Done remembers where the model was: ARKit's map of the room is saved with the placement, by the
design's name (Library/Application Support/SolidCraft/ar). Next time AR starts on that design it
recognises the room and puts the model back; a tap to place it again starts afresh.

Leaving the app and coming back keeps the model where it was: ARKit relocalizes, and the model is
hidden until it has. The app's own touch handling (long press menu, orbit) is off while AR shows.

## How it works

- `ios-device/AR/ARBridge.swift`: a framework the app loads at run time (as the AI's). It runs
  the ARKit session (world tracking, plane detection, LiDAR smoothed scene depth, light estimate),
  the coaching overlay and raycasts, and hands each frame over as plain floats and locked pixel
  buffers. Swift because ARKit's matrices are SIMD types the Rust bindings leave out.
- `src/ar.rs`: the AR view. Each frame it copies the camera picture (Y and CbCr planes) and the
  depth map into textures, draws the model with the design view's renderer from ARKit's camera
  (`Renderer::draw_second_view`, from VR pull request #18712), and lays it over the camera
  picture in one full-screen pass that also draws the contact shadow, hides the model behind
  nearer real things (LiDAR) and tints it with the room's light.
- The design view is drawn as ever under the AR view, so leaving AR changes nothing.

## Testing from the Mac

ARKit runs on the device only (not the simulator). Test steps (`SOLIDCRAFT_IOS_TEST`, see the
build notes): `ar`, `ar-off`, `ar-tap:x,y` (a finger tap, 0..1 across and down), `ar-pick:x,y`
(a Pencil tap), `ar-section`, `ar-measure`, `ar-explode`, `ar-pull:x0,y0,x1,y1` (a Pencil drag), `ar-photo`, `ar-joints`, `ar-check`, `ar-doubletap`,
`ar-squeeze:x,y` (points). The log (`tmp/solidcraft-last-error.txt`) has an `ar:` line for each tap and pick,
the first LiDAR depth map, thermal changes, and every 300 frames the CPU time and light gains.

## Limits

- The camera picture is shared with the GPU without a copy (CVMetalTextureCache wrapped by
  wgpu-hal, about 0.2 ms of CPU a frame); if that cannot be set up it is copied (about 1 ms).
- LiDAR's depth is 256 x 192, so edges of a hand in front of the model are soft.
- The contact shadow is not hidden by real things standing on it.
- At the thermal state "serious" the model is drawn at 60% of the pixels; at "critical" AR stops.
