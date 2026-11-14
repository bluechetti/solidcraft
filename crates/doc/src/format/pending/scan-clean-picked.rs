//! A Fill Holes step can keep the points by the holes that were clicked (`at`), and a Remove
//! Floating Pieces step the points on pieces clicked to keep or take off (`keep`, `remove`): the
//! step closes or takes off exactly those when the design opens. A step without them saves as
//! before, and an older design has none. The last release has no clean steps at all
//! (`scan-clean-feature.rs`): it warns about the newer format number and shows the scan as it came in.
