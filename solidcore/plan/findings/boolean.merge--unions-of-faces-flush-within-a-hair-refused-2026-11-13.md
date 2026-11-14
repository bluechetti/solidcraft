# boolean.merge: unions of bodies whose faces are flush within a hair are refused (2026-11-13)

- **Unit.** `boolean.merge_p2` (`make_faceuse`), `boolean.merge_p1`
  (`find_brep_region_of_other_face`), `topology.edge` (`find_radial_sector_between`).
- **Where it comes from.** SolidCraft's scan Reconstruct extrudes one prism per wall set it
  reads off a scan, each fitted on its own, and joins them. Prisms that share a floor or a
  wall come out flush within 1e-5 to 1.5e-3 mm and turned 4e-6 to 3e-5 rad off each other
  (2.5e-4 rad on one T-LESS part); some pairs are exactly parallel and 2.4e-5 to 3.8e-4 apart
  (the holes plate's counterbore floors). The body tolerance is 2e-5. Every refused join of
  the scan benchmark (families block 4 of 6 extrudes, holes plate, T-LESS scan-like 6, 17,
  21, 29: 12 booleans) is one such pair, refused in one of four ways:
  `make_faceuse: no radial edge use classifies the face's edge use` (5),
  `topology::brep::p6: no radial sector of the edge takes the new edge use` (2),
  `manifold_boolean: the union would delete face N and merge the solid it bounds into the
  outside` (1), and results the check refuses (`Face has Coincident Edges`, `Bad Loop
  Orient`, `Bad Gap` vertex/face just over half the tolerance) (4).
- **Reproduced from boxes.** A 60 x 40 x 17 plate and a block standing on it, the block's
  floor and sides offset by up to ±4e-4 and turned up to ±3e-5 rad about each axis: 23 of 300
  random unions fail, with the same four kinds of refusal (11 `make_faceuse`). One near
  coincidence at a time never fails (a block sunk, flush in or out, tilted on the top, a
  pocket floor beside another, offsets 1e-5 to 2e-3, angles 4e-6 to 2.5e-4: all exact). The
  failures need two or three near coincidences meeting at a corner. SolidCraft's test
  `coplanar_tests::a_block_flush_within_a_hair_with_a_plate_joins_into_one_solid` is one of
  them (listed in its tests/KNOWN_FAILING_TESTS).
- **What goes wrong in that case.** The block's wall y = -20 + 2.2e-4 crosses the plate's
  top face; the strip of the top face between the crossing (edge from (-30, -19.99945, 17)
  to (0, -19.99991, 17)) and the plate's side is 8.6e-5 to 5.5e-4 wide. When the block's
  wall above the top is copied in, `find_radial_sector_between` at that edge classifies it
  `Outside` against both candidate sectors at all ten parameters: the step-offs leave the
  strip. Before that, the region of the face is taken from the first mated edge's sector,
  which puts the block's outside face in the plate's solid region (edge 24 of the case),
  while its other edges' sectors say the infinite region.
- **Tried and dropped.** (1) Passing over a sector (or a sampled point) that puts a kept
  face in a region of the wrong class for the operation (void for a union, solid for an
  intersection or a difference) and asking the next edge: the face of the case is then
  placed in the infinite region but the sector at the strip edge still fails, and on another
  random case (`#230`) the union succeeds with the plate's whole volume missing (48000
  against 68400). A refusal is better than that, so it was not kept. (2) Raising every
  tolerance of both operands to 1e-4, 1e-3 or 1e-2 mm: 0, 1 and 4 of the 12 real cases pass
  and others change refusal; it is no fix.
- **What a fix needs.** A decision made once for the faces that lie within a few tolerances
  of each other across a strip (booleans.md §5's open "general strip rule"), extended from a
  crossing with one face's edge on the other to two or three such pairs meeting at a vertex;
  and a sector test that does not step off across a face narrower than its step (§4.2).
  The design-intent fix on SolidCraft's side is separate and simpler: Reconstruct snapping
  stacked prisms' directions and shared planes onto each other before extruding.
