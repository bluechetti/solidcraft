//! A kernel refusal said in words. The kernel names its failed checks (`SmEdge::Test #13 Bad
//! Edge loc ... [owner SmEdge]`); a person needs the reason and what to try, so every scan
//! command that builds a surface body reports a refusal through here rather than printing the
//! kernel's text.

use crate::EngineError;
use crate::params::bad;

/// Kinds of failure the kernel's check names: what to look for in its text, what it means.
const KINDS: [(&str, &str); 9] = [
    ("NonMonotonic", "the control net doubles back on itself"),
    ("CurveCusp", "an edge curve kinks sharply"),
    ("Bad Gap", "an edge does not lie on the faces it joins"),
    ("Bad Edge", "an edge does not lie on the faces it joins"),
    ("ntersect", "the surface passes through itself"),
    ("Tolerance", "two edges that should meet are further apart than the model's tolerance"),
    ("Degenerate", "part of the surface has collapsed to a point or a line"),
    ("Sliver", "a face is too thin to hold"),
    ("not supported", "the kernel does not support this shape yet"),
];

/// What to try, by the first kind of failure found: the fix that most often clears it.
const FIXES: [(&str, &str); 6] = [
    ("NonMonotonic", "fit a smaller region, or use fewer control points"),
    ("CurveCusp", "smooth the net near the kink (Edit Surface), or fit with more smoothing"),
    ("Bad Edge", "merge the surfaces with a smaller gap, or move the shared edge points onto each other"),
    ("Bad Gap", "merge the surfaces with a smaller gap, or move the shared edge points onto each other"),
    ("ntersect", "pull the folded part back (Edit Surface), or cut the region smaller"),
    ("Tolerance", "bring the edges together (Merge with a larger gap) before sewing"),
];

/// What the kernel's check refused, in words: each kind of failure it names, once. Text that
/// names no known kind says only that the check refused.
pub(super) fn kernel_words(kernel: &str) -> String {
    let mut said: Vec<&str> = Vec::new();
    for (name, words) in KINDS {
        if kernel.contains(name) && !said.contains(&words) {
            said.push(words);
        }
    }
    if said.is_empty() { "the kernel's check refused the joined surface".to_string() } else { said.join("; ") }
}

/// What to try for a refusal, from the first kind of failure it names; a refusal naming none
/// gets the general advice.
pub(super) fn kernel_fix(kernel: &str) -> &'static str {
    FIXES.iter().find(|(name, _)| kernel.contains(name)).map_or("try a smaller region, or fewer control points", |(_, fix)| fix)
}

/// A refusal of `what` (such as "the faces could not be sewn"): the reason in words, then what
/// to try. `kernel` is the kernel's own text, which is read here and never shown.
pub(super) fn refused(cmd: &str, what: &str, kernel: &str) -> EngineError {
    bad(cmd, format!("{what}: {}; {}", kernel_words(kernel), kernel_fix(kernel)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEAM: &str = "the operation failed: SMLib stitch: SMLib's check refuses the result: 2 failed tests:\nSmBSplineSurface::Test #5 Bad NonMonotonic CPts: SmBSplineSurface Control Net doubles back on itself probably making a self intersecting surface [owner SmBSplineSurface]\nSmBSplineCurve::Test #14 Bad CurveCusp: 3D BSplineCurve Warning: Extremely small radius of curvature (offset=0.0028879039, tolerance=0.0076947937) [owner SmBSplineCurve]";

    /// The kernel's refusal of a seam on T-LESS 01 (a wrap merged round on itself), in words: no
    /// test numbers, owners or offsets, each kind once.
    #[test]
    fn a_sewing_refusal_is_said_in_words() {
        let words = kernel_words(SEAM);
        assert_eq!(words, "the control net doubles back on itself; an edge curve kinks sharply; the surface passes through itself");
        assert!(!words.contains("Sm") && !words.contains('#'));
        assert_eq!(kernel_words("something else"), "the kernel's check refused the joined surface");
    }

    /// The error a person reads names the reason and a fix, and none of the kernel's text.
    #[test]
    fn a_refusal_gives_a_reason_and_a_fix_without_kernel_text() {
        let raw = "SmEdge::Test #13 Bad Edge loc: edge off its faces (offset=0.4, tolerance=0.01) [owner SmEdge]";
        let text = refused("scan.fit_patches", "the blocks could not be sewn", raw).to_string();
        assert!(text.contains("an edge does not lie on the faces it joins"), "{text}");
        assert!(text.contains("merge the surfaces with a smaller gap"), "{text}");
        for leak in ["SmEdge", "Test #", "owner", "offset="] {
            assert!(!text.contains(leak), "{leak} leaked: {text}");
        }
        let unknown = refused("scan.fit_patches", "the blocks could not be sewn", "odd").to_string();
        assert!(unknown.contains("try a smaller region"), "{unknown}");
    }
}
