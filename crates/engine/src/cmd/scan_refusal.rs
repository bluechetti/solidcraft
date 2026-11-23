//! A kernel refusal said in words. The kernel names its failed checks (`SmEdge::Test #13 Bad
//! Edge loc ... [owner SmEdge]`); a person needs the reason and what to try, so every scan
//! command that builds a surface body reports a refusal through here rather than printing the
//! kernel's text.

use crate::EngineError;
use crate::params::bad;

/// Kinds of failure the kernel's check names: what to look for in its text, what it means.
const KINDS: &[(&str, &str)] = &[
    ("NonMonotonic", "the control net doubles back on itself"),
    ("CurveCusp", "an edge curve kinks sharply"),
    ("Bad Gap", "an edge does not lie on the faces it joins"),
    ("Bad Edge", "an edge does not lie on the faces it joins"),
    ("ntersect", "the surface passes through itself"),
    ("Tolerance", "two edges that should meet are further apart than the model's tolerance"),
    ("Degenerate", "part of the surface has collapsed to a point or a line"),
    ("Sliver", "a face is too thin to hold"),
    ("not supported", "the kernel does not support this shape yet"),
    ("Bad Cone", "a slanted piece of the profile is too short to make a cone face"),
    ("SurfOfRevolution", "a turned face collapses where the profile meets the axis"),
    ("Face Near", "two faces meet a hair apart"),
    ("upstream status", "the faces could not be joined where they meet"),
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

/// Each kind of failure the kernel's text names, in words, once.
fn named_kinds(kernel: &str) -> Vec<&'static str> {
    let mut said: Vec<&str> = Vec::new();
    for (name, words) in KINDS {
        if kernel.contains(name) && !said.contains(words) {
            said.push(words);
        }
    }
    said
}

/// What the kernel's check refused, in words: each kind of failure it names, once. Text that
/// names no known kind says only that the check refused.
pub(super) fn kernel_words(kernel: &str) -> String {
    let said = named_kinds(kernel);
    if said.is_empty() { "the kernel's check refused the joined surface".to_string() } else { said.join("; ") }
}

/// What to try for a refusal, from the first kind of failure it names; a refusal naming none
/// gets the general advice.
pub(super) fn kernel_fix(kernel: &str) -> &'static str {
    FIXES.iter().find(|(name, _)| kernel.contains(name)).map_or("try a smaller region, or fewer control points", |(_, fix)| fix)
}

/// What to try when the kernel refuses a feature a Reconstruct step builds.
const RECONSTRUCT_FIX: &str = "undo to the last step that came back better and try the next proposal, read the part a region at a time (scan.reconstruct_selection), or build this feature by hand from scan.fit_plane, scan.fit_cylinder and scan.section_sketch";

/// A Reconstruct step's error, with a kernel refusal in it said in words and the steps' own fix;
/// any other error as it was.
pub(super) fn reconstruct_refusal(cmd: &str, e: EngineError) -> EngineError {
    let text = e.to_string();
    if !(text.contains("SMLib") || text.contains("solidcore")) {
        return e;
    }
    bad(cmd, format!("the kernel refused the feature this step builds: {}; {RECONSTRUCT_FIX}", kernel_words(&text)))
}

/// A refusal of `what` by a command that knows its own fix (Shell It: not the surface-fit advice
/// [`refused`] gives when the kernel's text names no known kind): the kinds named, if any, then
/// `fix`. `kernel` is read here and never shown.
pub(super) fn refused_trying(cmd: &str, what: &str, kernel: &str, fix: &str) -> EngineError {
    let said = named_kinds(kernel);
    if said.is_empty() { bad(cmd, format!("{what}; {fix}")) } else { bad(cmd, format!("{what}: {}; {fix}", said.join("; "))) }
}

/// A refusal of `what` (such as "the faces could not be sewn"): the reason in words, then what
/// to try. `kernel` is the kernel's own text, which is read here and never shown.
pub(super) fn refused(cmd: &str, what: &str, kernel: &str) -> EngineError {
    bad(cmd, format!("{what}: {}; {}", kernel_words(kernel), kernel_fix(kernel)))
}

/// A feature's kernel refusal said in words, with what to try, by the operation its text names:
/// `None` when the text is not the kernel's (no "SMLib" or "solidcore") or names none of them.
/// The kernel's text stays where it is; this is said after it. Kernel text like "make_faceuse:
/// no radial edge use classifies the face's edge use" left agents retrying the same join five
/// times (a replay of recorded agent calls, 2026-11-22).
pub fn feature_in_words(text: &str) -> Option<String> {
    if !(text.contains("SMLib") || text.contains("solidcore")) {
        return None;
    }
    let lower = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    let (what, fix) = if has(&["fillet", "blend", "rolling ball"]) {
        (
            "the round could not be built along these edges",
            "a smaller radius (the error names the largest that fits when it knows it), `tangent_chain: false`, the edges in smaller groups, or the round earlier in the timeline, before the feature that tightened its faces (timeline.roll_to, then the fillet, then roll forward)",
        )
    } else if has(&["draft"]) {
        (
            "the drafted faces' neighbours could not be extended to meet them",
            "a smaller angle, fewer faces drafted at once, or the draft before the rounds next to them",
        )
    } else if has(&["shell", "hollow", "thicken"]) {
        (
            "the walls could not be offset to that thickness",
            "a thinner wall, the shell before the rounds (fillet after shelling), or fewer faces removed",
        )
    } else if has(&["boolean", "union", "subtract", "intersect", "make_faceuse", "radial edge"]) {
        (
            "the bodies could not be combined where their faces meet (faces touching on a shared plane or along an edge, or a sliver between them, are the usual cause)",
            "let the tool overlap the body by a little (0.01 to 0.1 mm) instead of touching it on a face, run this step on its own, or change the order of the features",
        )
    } else if has(&["offset"]) {
        ("the faces could not be offset", "a smaller distance, or fewer faces at once")
    } else if has(&["sew", "stitch"]) {
        ("the faces could not be sewn into one body", "close the gaps between them first, or sew fewer faces at once")
    } else {
        return None;
    };
    Some(format!("in words: {what}; try {fix}"))
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

    /// A Reconstruct step the kernel refused (T-LESS 04's revolve before slivers were joined away)
    /// says why and what to try; an error of its own passes as it was.
    #[test]
    fn a_reconstruct_refusal_is_said_in_words() {
        let raw = bad(
            "scan.revolve_it",
            "body `x Reconstructed`: the operation failed: SMLib revolve: SMLib's check refuses the result: 1 failed tests:\nSmCone::Test #5 Bad Cone GenCurve [owner SmCone]",
        );
        let text = reconstruct_refusal("scan.revolve_it", raw).to_string();
        assert!(text.contains("too short to make a cone face") && text.contains("scan.reconstruct_selection"), "{text}");
        assert!(!text.contains("SmCone") && !text.contains("Test #"), "{text}");
        let own = reconstruct_refusal("scan.cut_it", bad("scan.cut_it", "there is no `x Reconstructed` yet")).to_string();
        assert!(own.contains("there is no") && !own.contains("kernel"), "{own}");
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

    /// A command with its own fix (Shell It) never gets the surface-fit advice, whatever the
    /// kernel's text names; a kind it does name is still said.
    #[test]
    fn a_refusal_with_its_own_fix_gives_that_fix() {
        let text = refused_trying("scan.shell_it", "the body could not be hollowed", "odd", "shell before the rounds").to_string();
        assert!(text.ends_with("the body could not be hollowed; shell before the rounds"), "{text}");
        assert!(!text.contains("control points") && !text.contains("check refused the joined surface"), "{text}");
        let text = refused_trying("scan.shell_it", "the body could not be hollowed", "Sliver face", "shell before the rounds").to_string();
        assert!(text.contains("a face is too thin to hold; shell before the rounds"), "{text}");
    }

    /// A feature's kernel refusal in words, by the operation its text names; text that is not
    /// the kernel's, or names no operation, gets nothing.
    #[test]
    fn a_feature_refusal_is_said_in_words_by_its_operation() {
        let boolean = "SMLib boolean: operation: solidcore: smlib_boolean_union_all: boolean::merge::p2::make_faceuse: no radial edge use classifies the face's edge use";
        let words = feature_in_words(boolean).unwrap_or_default();
        assert!(words.contains("could not be combined where their faces meet") && words.contains("overlap"), "{words}");
        let fillet = "SMLib fillet: operation: the blend failed (status 1001): the fillet body is not built: Fillet Solver Error";
        let words = feature_in_words(fillet).unwrap_or_default();
        assert!(words.contains("round could not be built") && words.contains("tangent_chain: false"), "{words}");
        let draft = "SMLib draft: operation: the offset is refused: how far its neighbours extend to meet it has no certified bound";
        assert!(feature_in_words(draft).is_some_and(|w| w.contains("drafted faces")), "draft before offset");
        assert_eq!(feature_in_words("no body `Housing`"), None);
        assert_eq!(feature_in_words("SMLib something else entirely"), None);
    }
}
