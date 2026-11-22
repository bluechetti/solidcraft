//! A design saves `scan_tolerances`: for a scan (a mesh body), how far off its surface still counts
//! as on it, in mm, the one tolerance every Scan-tab tool colours and reports against. An older
//! design has none and every scan uses the default (0.1 mm). Older designs need no transformation.
//! The last release warns about the newer format number, opens the design without these, and keeps
//! them on save.
