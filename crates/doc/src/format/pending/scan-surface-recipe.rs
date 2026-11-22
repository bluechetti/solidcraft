//! A Scan Surface feature may save a `recipe`: the scan it was fitted to, the command that made it
//! (`scan.free_form` or `scan.fit_patches`) with the exact parameters it was given, and where the fit
//! put each control point (the baseline the hand edits are measured from). `scan.refit` makes the
//! surface again from it on the scan as it is now. An older design has none: its Scan Surfaces are
//! plain surfaces, as before. Older designs need no transformation. The last release warns about
//! the newer format number, opens the design without recipes, and keeps them on save.
