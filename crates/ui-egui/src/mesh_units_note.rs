//! The note offering to read a mesh again in its file's own unit. STL, OBJ and PLY files don't
//! say what unit they are in: the dialog of [`crate::mesh_units`] asks before a person's file comes
//! in, millimetres first, and a script or agent gives `units` or none for millimetres. A scan saved
//! in metres and read as millimetres comes in a thousand times too small (the Stanford bunny 0.16
//! mm across). When a mesh file just inserted or opened in millimetres comes in under [`TINY_MM`]
//! across, a note over the view (the shared toast, as the stand-up note) says how big it is and
//! offers Meters or Inches: `mesh.units` scales the import where it is, in one undo step. It goes
//! by itself after [`NOTE_SECONDS`], or as soon as the design changes; Preferences › Design ›
//! "Offer to scale a mesh that comes in tiny" turns it off. A file read in another unit its caller
//! chose gets no note, and nor do 3MF and glTF files, which state their own.

use serde_json::{Value, json};

use crate::SolveApp;

/// A mesh smaller than this across (the longest side of its box, in mm) is taken to be in another
/// unit: a part scanned or printed is rarely so small, while one saved in metres (up to 3 m) or
/// in inches (up to 3 in) reads this small as millimetres.
pub const TINY_MM: f64 = 3.0;

/// How long the note stays when nobody is pointing at it, in seconds.
pub const NOTE_SECONDS: f64 = 15.0;

/// A mesh that came in tiny, waiting on the note.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    /// A body of the import: `mesh.units` scales every body of it.
    pub body: String,
    /// The import feature: the note stays only while it is the last in the timeline.
    pub feature: u64,
    /// Its longest side, in mm.
    pub across: f64,
    /// The design's revision when the note was first drawn: any change after takes it away.
    pub revision: Option<u64>,
    /// When the note was first drawn, or last pointed at.
    pub since: Option<f64>,
}

/// After a file was inserted or opened (its command's `result`), offer to read it in another unit
/// when it is a mesh read in millimetres (the unit's dialog left on them, or no unit given) that
/// came in under [`TINY_MM`] across. A file read in another unit is as it was asked for.
pub fn offer(app: &mut SolveApp, path: &str, result: Option<&Value>) {
    app.mesh_units_note = None;
    let io = &solidcraft_engine::io::is_mesh_path;
    let states_its_units = solidcraft_engine::io::is_gltf_path(path)
        || std::path::Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("3mf"));
    if !app.preferences.offer_mesh_units || states_its_units || !(io(path) || solidcraft_engine::io::is_ply_path(path)) {
        return;
    }
    let read_as_mm = |r: &&Value| r.get("units").and_then(Value::as_str).is_none_or(|u| u == "mm");
    let Some(result) = result.filter(read_as_mm) else { return };
    let bodies: Vec<&str> = result.get("bodies").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
    let state = app.session.model.state();
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    let mut feature = None;
    for name in &bodies {
        let Some(b) = state.body(name) else { continue };
        feature = feature.or(Some(b.feature));
        for p in b.body.triangle_mesh().map(|(p, _)| p).unwrap_or_default() {
            for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
                lo[k] = lo[k].min(v);
                hi[k] = hi[k].max(v);
            }
        }
    }
    let across = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
    let (Some(body), Some(feature)) = (bodies.first(), feature) else { return };
    if across.is_finite() && across > 0.0 && across < TINY_MM {
        app.mesh_units_note = Some(Offer { body: (*body).to_string(), feature, across, revision: None, since: None });
    }
}

/// The note is waiting. No side effects.
pub(crate) fn note_pending(app: &SolveApp) -> bool {
    app.mesh_units_note.is_some()
}

/// Is the mesh still as it came in: its import the last feature, its body there, and the design
/// unchanged since the note was first drawn? No side effects.
fn current(app: &SolveApp, offer: &Offer) -> bool {
    app.session.doc.features.last().map(|f| f.id) == Some(offer.feature)
        && app.session.model.state().body(&offer.body).is_some()
        && offer.revision.is_none_or(|r| r == app.session.revision)
}

/// The note due this frame, as its text, for the shared toast (`viewport::transient_messages`) to
/// draw with the buttons [`note_labels`]. Its clock starts the first frame it is drawn; after
/// [`NOTE_SECONDS`] without the pointer on it (`note_keep`), or once the design changes, it goes,
/// leaving the mesh as it came in.
pub(crate) fn note_due(app: &mut SolveApp, ctx: &egui::Context) -> Option<String> {
    let offer = app.mesh_units_note.as_ref()?;
    if !current(app, offer) {
        app.mesh_units_note = None;
        return None;
    }
    let now = ctx.input(|i| i.time);
    let revision = app.session.revision;
    let offer = app.mesh_units_note.as_mut()?;
    offer.revision.get_or_insert(revision);
    let since = *offer.since.get_or_insert(now);
    let shown = now - since;
    if !shown.is_finite() || shown >= NOTE_SECONDS {
        app.mesh_units_note = None;
        return None;
    }
    let size = format!("{} mm", two_figures(offer.across));
    Some(solidcraft_ui_text::format("ui.mesh_units_note", &[("size", &size)]))
}

/// `v` to two significant figures: 0.16, 2.5, 0.0042.
fn two_figures(v: f64) -> String {
    let decimals = (1.0 - v.log10().floor()).clamp(0.0, 12.0) as usize;
    format!("{v:.decimals$}")
}

/// The handles of the note's buttons, in order: Meters, Inches, then turning the offer off.
pub const NOTE_HANDLES: [&str; 3] = ["mesh_units_note:meters", "mesh_units_note:inches", "mesh_units_note:never"];

/// The handle of the note's close (×), which leaves the mesh as it came in.
pub const NOTE_CLOSE: &str = "toast:mesh_units_note:close";

/// The note's buttons, in the order of [`NOTE_HANDLES`], and the unit each reads the mesh in.
pub(crate) fn note_labels() -> [(String, Option<&'static str>); 3] {
    [
        (solidcraft_ui_text::tr("ui.mesh_units_meters").to_string(), Some("m")),
        (solidcraft_ui_text::tr("ui.mesh_units_inches").to_string(), Some("in")),
        (solidcraft_ui_text::tr_label("Don't ask again").to_string(), None),
    ]
}

/// The share of the note's time left now, 1 to 0, for the countdown ring around its close (×).
pub(crate) fn note_left(app: &SolveApp, ctx: &egui::Context) -> f32 {
    let now = ctx.input(|i| i.time);
    app.mesh_units_note.as_ref().and_then(|o| o.since).map_or(1.0, |since| crate::viewport::countdown_left(since, now, NOTE_SECONDS))
}

/// The note was drawn at `rect`: a pointer on it keeps it from going.
pub(crate) fn note_keep(app: &mut SolveApp, ctx: &egui::Context, rect: egui::Rect) {
    if ctx.input(|i| i.pointer.hover_pos()).is_some_and(|pos| rect.contains(pos))
        && let Some(offer) = app.mesh_units_note.as_mut()
    {
        offer.since = Some(ctx.input(|i| i.time));
    }
}

/// The note's button `index` (of [`NOTE_HANDLES`]) was clicked.
pub(crate) fn note_act(app: &mut SolveApp, index: usize) {
    let Some(offer) = app.mesh_units_note.take() else { return };
    match note_labels().get(index).map(|(_, units)| *units) {
        Some(Some(units)) => match app.run("mesh.units", json!({ "units": units, "body": offer.body })) {
            Ok(_) => app.fit_view(),
            Err(e) => app.set_status(e, true),
        },
        // Preferences › Design turns the offer back on.
        Some(None) => app.preferences.offer_mesh_units = false,
        None => {}
    }
}

/// Close the note, leaving the mesh as it came in.
pub fn leave(app: &mut SolveApp) {
    app.mesh_units_note = None;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// A triangle `side` across, as an STL file in `dir`.
    fn triangle(dir: &std::path::Path, name: &str, side: f64) -> String {
        let path = dir.join(name).to_string_lossy().into_owned();
        std::fs::write(
            &path,
            format!("solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex {side} 0 0\nvertex 0 {side} 0\nendloop\nendfacet\nendsolid t\n"),
        )
        .unwrap();
        path
    }

    fn across(h: &crate::scenario::Harness, body: &str) -> f64 {
        let state = h.app.session.model.state();
        let (p, _) = state.body(body).unwrap().body.triangle_mesh().unwrap();
        p.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max) - p.iter().map(|p| p.x).fold(f64::INFINITY, f64::min)
    }

    #[test]
    fn a_mesh_that_comes_in_tiny_is_offered_meters_or_inches() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        let tiny = triangle(&h.dir, "tiny.stl", 0.15);
        h.app.insert_path(&tiny);
        h.frames(3);
        let offer = h.app.mesh_units_note.clone().expect("a note for a mesh 0.15 mm across");
        assert_eq!(offer.body, "tiny");
        assert!((offer.across - 0.15).abs() < 1e-6);
        assert!(crate::scenario::handle_at("mesh_units_note:meters").is_some(), "the note's buttons are drawn");
        assert_eq!(super::two_figures(offer.across), "0.15");
        // Meters: read again 1000 times larger, in one undo step.
        let undo = h.app.session.undo.len();
        h.step(&json!({"click": {"handle": "mesh_units_note:meters"}})).unwrap();
        h.frames(2);
        assert!(h.app.mesh_units_note.is_none());
        assert!((across(&h, "tiny") - 150.0).abs() < 1e-3, "{}", across(&h, "tiny"));
        assert_eq!(h.app.session.undo.len(), undo + 1);
        // Inches, on another tiny mesh.
        let small = triangle(&h.dir, "small.stl", 2.0);
        h.app.insert_path(&small);
        h.frames(3);
        h.step(&json!({"click": {"handle": "mesh_units_note:inches"}})).unwrap();
        assert!((across(&h, "small") - 50.8).abs() < 1e-3, "{}", across(&h, "small"));
    }

    #[test]
    fn no_note_for_a_mesh_of_a_real_size_given_units_or_after_a_change() {
        let mut h = crate::scenario::Harness::new();
        h.app.home.open = false;
        // A mesh 20 mm across is as it should be.
        h.app.insert_path(&triangle(&h.dir, "part.stl", 20.0));
        h.frames(2);
        assert!(h.app.mesh_units_note.is_none());
        // Read in another unit its caller chose (0.015 cm is 0.15 mm): as it was asked for.
        let tiny = triangle(&h.dir, "tiny.stl", 0.015);
        let r = h.app.run("file.insert_mesh", json!({"path": tiny, "units": "cm"})).unwrap();
        super::offer(&mut h.app, &tiny, Some(&r));
        assert!(h.app.mesh_units_note.is_none());
        // Millimetres, as the unit's dialog opens on, still get the note.
        let r = h.app.run("file.insert_mesh", json!({"path": tiny, "units": "mm"})).unwrap();
        super::offer(&mut h.app, &tiny, Some(&r));
        assert!(h.app.mesh_units_note.is_some());
        super::leave(&mut h.app);
        // A change to the design takes the note away, leaving the mesh as it came in.
        h.app.insert_path(&triangle(&h.dir, "tiny2.stl", 0.15));
        h.frames(2);
        assert!(h.app.mesh_units_note.is_some());
        h.app.run("solid.box", json!({"length": 5, "width": 5, "height": 5})).unwrap();
        h.frames(2);
        assert!(h.app.mesh_units_note.is_none());
        assert!((across(&h, "tiny2") - 0.15).abs() < 1e-6);
        // The close (×), and Don't ask again, which turns the offer off.
        h.app.insert_path(&triangle(&h.dir, "tiny3.stl", 0.15));
        h.frames(2);
        h.step(&json!({"click": {"handle": "mesh_units_note:never"}})).unwrap();
        assert!(h.app.mesh_units_note.is_none() && !h.app.preferences.offer_mesh_units);
        h.app.insert_path(&triangle(&h.dir, "tiny4.stl", 0.15));
        h.frames(2);
        assert!(h.app.mesh_units_note.is_none(), "turned off in Preferences");
        assert!((across(&h, "tiny4") - 0.15).abs() < 1e-6);
    }
}
