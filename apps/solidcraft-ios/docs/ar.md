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
(a Pencil tap). The log (`tmp/solidcraft-last-error.txt`) has an `ar:` line for each tap and pick,
the first LiDAR depth map, thermal changes, and every 300 frames the CPU time and light gains.

## Limits

- The camera picture is copied each frame (about 1 ms of CPU); no zero-copy yet.
- LiDAR's depth is 256 x 192, so edges of a hand in front of the model are soft.
- The contact shadow is not hidden by real things standing on it.
- At the thermal state "serious" the model is drawn at 60% of the pixels; at "critical" AR stops.
