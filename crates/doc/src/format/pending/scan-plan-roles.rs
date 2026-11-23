//! A scan's plan says what the model is for (`purpose`: replacement or copy), and each of its
//! regions may say what the user marked it as (`role`: focus, hold, ignore or keep), the size held
//! there (`hold`, mm) and the kept scan selection that is its extent (`selection`, a name in the
//! design's `scan_selections`). All are optional: older designs have none, and read as before. The
//! last release opens a design with them with a warning about the newer format and keeps what it
//! can't read.
