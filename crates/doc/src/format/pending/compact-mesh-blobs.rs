//! A design saves an imported mesh's blob compactly (`SCMESH03`) when it has no shading normals:
//! its positions as f32 when every coordinate is exactly one (a scan's from an STL or PLY file
//! always is), else f64 as before, and each triangle corner as the zigzag LEB128 of its difference
//! from the corner before. A 28 million-triangle scan saves in 207 MB instead of 262 MB, within
//! the design file limit with room to spare. A mesh with shading normals is saved as `SCMESH02`
//! as before; `SCMESH01` blobs are still read, and so is a compact blob saved under `SCMESH02` by
//! the Scan workspace before shaded blobs took that magic. The last release refuses a design with
//! this change (the newer format number).
