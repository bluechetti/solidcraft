//! A design saves a `scan_surface` feature for a smooth surface made on the Scan tab (Fit Surface,
//! New Free Form, Wrap Surface, Merge Surfaces, Fit Patches): its file name
//! and its STEP text, as an Import feature keeps them, but a feature kind of its own, so the
//! history, the browser and Edit Surface know it for what it is. An older design keeps these
//! surfaces in `import` features, which still open and edit exactly as before; no older design is
//! rewritten. The last release opens a design with this change with a warning (the newer format
//! number), keeps the feature as the file has it, and cannot build its surface.
