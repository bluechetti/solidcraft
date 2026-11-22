//! A design saves a `scan_clean` feature for each clean tool run on a scan (Remove Floating Pieces,
//! Fill Holes, Reduce Mesh, Repair Mesh, Denoise, Remesh): the scan's body name and the tool's
//! settings, never the triangles it makes. The scan stays in its Mesh Import as it came in, and the
//! cleaned scan is made again from it, step by step, when the design opens. An older design has no
//! such features: its scans were cleaned by rewriting their Mesh Import, which still opens exactly
//! as it was saved, and no older design is rewritten. The last release opens a design with this
//! change with a warning (the newer format number), keeps the features as the file has them, and
//! shows the scan as it came in, uncleaned.
