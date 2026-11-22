//! A design saves `scan_flat_patterns`: for each Flat Pattern sketch made by Flatten, the
//! flattening that made it (the scan, the region, and the outline's curves), so Roll onto Scan can
//! lift what is drawn on the pattern back onto the scan after the design is opened again. Before,
//! this lived only while the app was open. An older design has none: Roll onto Scan asks for the
//! region again (flatten it again, or give its point, radius or triangles). Older designs need no
//! transformation. The last release warns about the newer format number, opens the design without
//! these, and keeps them on save.
