//! A Scan Clean step can be Remove and Fill (`op: remove_and_fill`): the `area` of the scan taken
//! off (the triangles of the scan as the step meets it, as runs, as other steps keep an area) and
//! `fair`. When the design opens the step takes those triangles off the scan before it and closes
//! the holes that leaves. An older design has no such step and is not rewritten. The last release
//! opens a design with one with a warning about the newer format, keeps the step it can't read and
//! shows the scan without it.
