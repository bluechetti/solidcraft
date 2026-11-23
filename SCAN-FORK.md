# Scan to CAD: a work-in-progress fork of SolidCraft

This branch is [SolidCraft](https://github.com/tangency-labs/solidcraft) (upstream `main` at
`bd46bd378`, after v0.46.2) with a Scan workspace added on top: tools that turn a 3D scan (a mesh
or point cloud) into CAD. It is shared so it can be built and tried. It is not an upstream release,
and nothing here is endorsed by the SolidCraft maintainers.

## Build and run

The same as upstream (see README.md and docs/building.md):

```sh
cargo run --release -p solidcraft            # the desktop app; Workspace › Scan
cargo run --release -p solidcraft-cli -- commands | grep '^scan\.'
```

Open or insert a scan (STL, OBJ, PLY, 3MF, or a point cloud: PTX, E57, LAS/LAZ, PLY, PCD, XYZ),
switch to the Scan workspace and start with **Survey Scan**: it reads the scan and proposes what to
run next.

## Two routes

- **Rebuild** (`scan.reconstruct`, with Find All): for machined, prismatic parts. Finds planes,
  cylinders, holes and cuts, and builds an editable solid of exact faces.
- **Wrap Scan** (`scan.wrap_scan`): for cast and organic parts. Wraps the whole scan in one network
  of patches sewn into a closed solid.

Around them: clean-up (repair, reduce, denoise, fill holes, remove floating parts), alignment and
registration, fitting (plane, cylinder, sphere, cone, torus, free-form surfaces), sections and
curves on the scan, deviation maps and a verdict against the scan, and a report.

## Where it stands

Works well:
- Machined parts from synthetic noisy scans: 9–10 exact faces within 0.007 mm of the true part.
- Wrap Scan on a real Artec industrial part: 99.9 % of the surface within 0.2 mm, volume off by
  0.04 %.
- Large scans: tens of millions of triangles import and display (28M triangles: 7 s, 3.2 GB).

Not there yet:
- Organic parts with many details: a car rim reaches 97.8 % within 0.2 mm (other tools reach
  99.5 % at the same patch count); bodies above ~40,000 patches cannot be saved yet.
- Some scans are refused (a water pump with an edge shared by three faces).
- Three of six clean CAD-export test parts do not rebuild correctly yet (block, holes, gear).
- Wrapped surfaces are freeform patches, not editable features.
- The click count from scan to STEP is high (18), and the verdict is not shown in the window.

## Differences from upstream you may notice

- Scans, point clouds and designs have no size limit on the desktop.
- Fit Curves to Mesh Section on a mesh body uses the Scan workspace's section fit (within a scan's
  noise), not upstream's STL section fit (#20659). Upstream's test for that is listed in
  `tests/KNOWN_FAILING_TESTS`.
- A large mesh (over 50,000 triangles) is highlighted on the GPU rather than copied.

## Credits

- SolidCraft and its contributors: everything this builds on (MIT OR Apache-2.0; the look and the
  name under LICENSE-BRAND).
- [d0pawlus/SurfaceCraft](https://github.com/d0pawlus/SurfaceCraft) (MIT OR Apache-2.0): Wrap
  Scan's rounds and quad layout, the held mesh reduction and the handle closer are adapted from it;
  see `attribution/pending/scan-wrap-scan.md`.
- "SurfaceCraft" elsewhere in `crates/scan` means this fork's author's own earlier, private app
  of that name, from which most of the scan tools were ported.
- The scans in the documentation videos are credited in `attribution/pending/` (T-LESS and YCB
  under CC BY 4.0, the Stanford 3D Scanning Repository, Smithsonian Open Access CC0).
