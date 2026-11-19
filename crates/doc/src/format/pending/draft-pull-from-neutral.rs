//! A Draft may save a pull of `[0, 0, 0]`: it pulls along its neutral plane's normal as that
//! plane is when the design is made again, so a draft whose neutral plane follows a hinge (Tilt
//! Face: the face's own plane turned a right angle about an edge or axis) keeps pulling the way the
//! plane now faces. Older designs always save a pull and are read as before. The last release
//! opens a design with such a draft with a warning about the newer format; the draft fails there
//! (a pull of no direction) and the rest is kept.
