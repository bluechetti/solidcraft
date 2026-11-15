//! A feature a scan command makes (a surface fitted to a scan, a solid reconstructed from it, a
//! piece split off it, a section sketch, a fitted plane) may save `made_from`: the scans it was
//! made from. A Move of such a scan moves the bodies made from it too and places those sketches
//! and planes again by the same move, so they stay on the scan they were made from. An older
//! design has none: its scans move alone, as before. Older designs need no transformation. The
//! last release warns about the newer format number and opens the design without the field: its
//! scans move alone there.
