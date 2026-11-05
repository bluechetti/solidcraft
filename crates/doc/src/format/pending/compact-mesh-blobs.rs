//! A design saves an imported mesh's blob compactly (`SCMESH02`): its positions as f32 when every
//! coordinate is exactly one (a scan's from an STL or PLY file always is), else f64 as before, and
//! each triangle corner as the zigzag LEB128 of its difference from the corner before. A 28
//! million-triangle scan saves in 207 MB instead of 262 MB, within the design file limit with room
//! to spare. An older design's `SCMESH01` blobs are still read. The last release refuses a design
//! with this change (the newer format number).
