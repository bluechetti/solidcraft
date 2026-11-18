//! A Remesh step of a scan can keep an `offset` (mm): the scan made again that far off its surface,
//! positive the way its faces point (a limb grown for a liner, shrunk for a socket's fit). Older
//! designs have no offset: their Remesh steps make the surface on the scan, as they always did.
//! The last release opens a design with an offset with a warning about the newer format and keeps
//! what it can't read.
