//! A design saves `scan_plans`: for a scan (a mesh body), what it is being turned into: how the part
//! was made (`process`), the share of the scan and of the model that must be within the scan's
//! tolerance of each other (`target`), and the regions of the part, each open, built (by a feature),
//! skipped or inferred (each with a reason). Scan Verdict reads it. An older design has none and its
//! scans have no plan (the verdict then reads the process's default target and says so). Older
//! designs need no transformation. The last release warns about the newer format number, opens the
//! design without these, and keeps them on save.
