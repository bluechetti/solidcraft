//! A Scan Clean step can be Remove Markers (`op: remove_markers`): the reference spheres' `radius`
//! and the `centres` found when the step was made, and `fair`. When the design opens the step takes
//! those spheres off the scan before it and closes the holes they leave. An older design has no
//! such step and is not rewritten. The last release has no clean steps at all
//! (`scan-clean-feature.rs`): it warns about the newer format number and shows the scan as it came in.
