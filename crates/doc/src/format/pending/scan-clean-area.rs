//! A Denoise, Reduce Mesh or Remesh step of a scan can work on part of the scan: the step keeps an
//! `area`, the triangles of the scan as the step meets it, as runs of consecutive ones
//! (`{"triangles": count, "runs": [[first, how many], …]}`). Older designs have no area: their
//! steps work on the whole scan, as they always did. The last release opens a design with an
//! area with a warning about the newer format and keeps what it can't read.
