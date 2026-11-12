//! Flat Template (`scan.flat_template`): click the corners of the template on the wall's scan, in
//! order round it. Only those points are used; the template is laid flat on the wall's plane
//! (fitted to the scan, what stands off it left out), with a cut-out for each part standing off the
//! plane, and drawn over the view before OK sketches it: the outline, the cut-outs, and the scan's
//! own edge on the same plane to check it against.

use std::sync::Arc;

use serde_json::{Value, json};
use solidcore_record::{Vec2, Vec3};
use solidcraft_engine::Sel;
use solidcraft_engine::cmd::scan::template::prepare;
use solidcraft_engine::geom::prelude::*;

use crate::SolveApp;
use crate::dialogs::{Kind, check, form_row};
use crate::scan_job;
use crate::selection::{FACES, SelInput, items};
use crate::ui_kit::{self, choice_row};

/// The Flat Template dialog's values (the clicks on the scan are its selection input).
#[derive(Clone, Debug)]
pub struct FlatTemplate {
    /// Which parts are cut out: both sides, out, in.
    pub side: usize,
    /// How far off the plane a part must stand; 0 for what the wall's noise says.
    pub tolerance: f64,
    pub edge: bool,
    /// The scan is coloured by how far it stands off the template's plane (on by default).
    pub colours: bool,
    /// A tolerance tried here instead of the scan's own, mm (0: the scan's, see `scan.tolerance`).
    pub scan_tolerance: f64,
    /// The scan's tolerance as changed here, kept by OK.
    keep_tolerance: Option<(String, f64)>,
    /// The template being worked out beside the window (a large scan), for the values it was begun for.
    pending: Option<Arc<scan_job::Job<String, Computed>>>,
    shown: Option<Box<Shown>>,
}

impl Default for FlatTemplate {
    fn default() -> Self {
        FlatTemplate { side: 0, tolerance: 0.0, edge: true, colours: true, scan_tolerance: 0.0, keep_tolerance: None, pending: None, shown: None }
    }
}

#[derive(Clone, Debug)]
struct Shown {
    key: String,
    result: Result<Marked, String>,
    under: Option<Under>,
}

/// The scan over the template, each point by how far it stands off the template's plane (along the
/// side `Out` goes), and the coloured dots made for a tolerance and colours.
#[derive(Clone, Debug)]
struct Under {
    scan: String,
    at: Vec<Vec3>,
    values: Vec<f32>,
    /// Whether the point is inside a cut-out: a part standing off the wall by design, which does not
    /// count against how flat the wall is.
    cut: Vec<bool>,
    dots: Option<(u64, [[u8; 3]; 3], u64, Arc<Vec<u8>>)>,
}

/// The colouring's id among the clouds the viewport draws, and the most points it shows.
const SHADE_CLOUD_ID: u64 = u64::MAX - 12;
const SHADE_POINTS: usize = 40_000;

/// The template as it will lie, in space.
#[derive(Clone, Debug)]
struct Marked {
    outline: Vec<Vec3>,
    cutouts: Vec<Vec<Vec3>>,
    edge: Vec<Vec<Vec3>>,
    /// The wall's plane as a ghost over the outline, a little larger than it.
    ghost: [Vec3; 4],
    /// Each side of the outline: its middle and its length (mm), the dimension between two
    /// neighbouring points picked.
    sides: Vec<(Vec3, f64)>,
    area: f64,
    cut_area: f64,
    tolerance: f64,
    fitted: bool,
    kept: usize,
    under: usize,
}

const SIDES: [&str; 3] = ["both", "out", "in"];

pub fn start(_app: &SolveApp, id: &str) -> Option<(Kind, Vec<SelInput>)> {
    (id == "scan.flat_template")
        .then(|| (Kind::ScanFlatTemplate(FlatTemplate::default()), vec![SelInput::new("Outline Points", FACES, true).places()]))
}

fn tr(id: &'static str) -> &'static str {
    crate::i18n::tr(id)
}

/// What `scan.flat_template` is sent: the clicks in order, once there are three.
fn params(k: &FlatTemplate, inputs: &[SelInput]) -> Option<Value> {
    let clicks: Vec<(&str, Vec3)> = items(inputs, 0)
        .iter()
        .filter_map(|s| match s {
            Sel::Face { body, point, .. } => Some((body.as_str(), *point)),
            _ => None,
        })
        .collect();
    let (body, _) = *clicks.first()?;
    if clicks.len() < 3 {
        return None;
    }
    let mut p = json!({
        "body": body,
        "outline": clicks.iter().map(|(_, q)| json!([q.x, q.y, q.z])).collect::<Vec<_>>(),
        "side": SIDES[k.side.min(SIDES.len() - 1)],
        "show_scan_edge": k.edge,
    });
    if k.tolerance > 0.0 {
        p["tolerance"] = json!(k.tolerance);
    }
    Some(p)
}

/// Work the template out again when the clicks, the values or the design have changed.
fn look(app: &SolveApp, k: &mut FlatTemplate, inputs: &[SelInput]) {
    let Some(p) = params(k, inputs) else {
        k.shown = None;
        k.pending = None;
        return;
    };
    let key = format!("{p}|{}", app.session.revision);
    if k.shown.as_ref().is_some_and(|s| s.key == key) {
        k.pending = None;
        return;
    }
    // On a large scan the template is worked out beside the window and what was drawn stays until it arrives.
    if scan_job::beside_window(app) {
        if let Some(job) = k.pending.clone() {
            match job.poll() {
                scan_job::Poll::Working if job.key == key => return,
                scan_job::Poll::Done((result, under)) if job.key == key => {
                    k.pending = None;
                    k.shown = Some(Box::new(Shown { key, result, under }));
                    return;
                }
                _ => k.pending = None,
            }
        }
        let given = p.clone();
        if let Some(job) = scan_job::spawn(app, key.clone(), move |s| compute(s, &given)) {
            k.pending = Some(Arc::new(job));
            return;
        }
    }
    let (result, under) = compute(&app.session, &p);
    k.shown = Some(Box::new(Shown { key, result, under }));
}

/// The template `p` asks for and the scan over it: all that looks over the scan.
type Computed = (Result<Marked, String>, Option<Under>);

fn compute(s: &solidcraft_engine::Session, p: &Value) -> Computed {
    let mut under = None;
    let result = prepare(s, p, "scan.flat_template").map_err(|e| e.to_string()).map(|m| {
        under = under_of(s, &m);
        let t = &m.made;
        let place = |ring: &[solidcore_record::Vec2]| -> Vec<Vec3> { ring.iter().map(|q| t.at(*q)).collect() };
        let (lo, hi) = t.outline.iter().fold((Vec2::new(f64::MAX, f64::MAX), Vec2::new(f64::MIN, f64::MIN)), |(lo, hi), q| {
            (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y)))
        });
        let grow = 0.15 * (hi.x - lo.x).max(hi.y - lo.y);
        let (lo, hi) = (Vec2::new(lo.x - grow, lo.y - grow), Vec2::new(hi.x + grow, hi.y + grow));
        let sides = t.outline.iter().zip(t.outline.iter().cycle().skip(1)).map(|(a, b)| (t.at((*a + *b) / 2.0), a.dist(*b))).collect();
        Marked {
            ghost: [t.at(lo), t.at(Vec2::new(hi.x, lo.y)), t.at(hi), t.at(Vec2::new(lo.x, hi.y))],
            sides,
            outline: place(&t.outline),
            cutouts: t.cutouts.iter().map(|c| place(&c.ring)).collect(),
            edge: if m.show_scan_edge { t.scan_edge.iter().map(|l| place(l)).collect() } else { Vec::new() },
            area: t.area,
            cut_area: t.cut_area,
            tolerance: m.tolerance,
            fitted: m.fitted,
            kept: m.kept,
            under: m.under,
        }
    });
    (result, under)
}

/// Whether `p` is inside the ring (even-odd), both in the template's coordinates.
fn inside(ring: &[Vec2], p: Vec2) -> bool {
    let mut on = false;
    for (a, b) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            on = !on;
        }
    }
    on
}

/// The scan's points over the template's outline and how far each stands off its plane: thinned
/// evenly to [`SHADE_POINTS`].
fn under_of(s: &solidcraft_engine::Session, m: &solidcraft_engine::cmd::scan::template::Prepared) -> Option<Under> {
    let state = s.model.state();
    let (positions, _) = state.body(&m.body)?.body.triangle_mesh()?;
    let t = &m.made;
    let normal = t.frame.x.cross(t.frame.y).normalized()?;
    let step = (positions.len() / SHADE_POINTS).max(1);
    let local = |p: Vec3| {
        let d = p - t.frame.origin;
        Vec2::new(d.dot(t.frame.x), d.dot(t.frame.y))
    };
    let mut under = Under { scan: m.body.clone(), at: Vec::new(), values: Vec::new(), cut: Vec::new(), dots: None };
    for p in positions.iter().step_by(step) {
        let q = local(*p);
        if !inside(&t.outline, q) {
            continue;
        }
        under.at.push(*p);
        under.values.push(((*p - t.frame.origin).dot(normal)) as f32);
        under.cut.push(t.cutouts.iter().any(|c| inside(&c.ring, q)));
    }
    (!under.at.is_empty()).then_some(under)
}

/// The scan's dots made again when the tolerance or the colours have changed.
fn colour(app: &SolveApp, k: &mut FlatTemplate) {
    let colours = crate::scan_deviation::colours();
    let Some(under) = k.shown.as_mut().and_then(|s| s.under.as_mut()) else { return };
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.scan_tolerance);
    if under.dots.as_ref().is_some_and(|(t, c, _, _)| *t == tolerance.to_bits() && *c == colours) {
        return;
    }
    let mut out = Vec::with_capacity(under.at.len() * crate::scan_dots::DOT_SIZE);
    for (at, v) in under.at.iter().zip(&under.values) {
        let Some(s) = crate::dialogs_scan_surface::solidcraft_scan_scale(*v, tolerance) else { continue };
        let dot = crate::scan_dots::Dot { at: at.to_f32(), value: *v, rgb: crate::scan_deviation::colour(colours, s) };
        crate::scan_dots::cloud_dot(&mut out, &dot);
    }
    under.dots = Some((tolerance.to_bits(), colours, crate::dialogs_scan_surface::next_shade_version(), Arc::new(out)));
}

/// How flat the wall is under the template, and the tolerance: the points outside the cut-outs,
/// for the viewport's key and the dialog's line.
pub(crate) fn fidelity(app: &SolveApp, k: &FlatTemplate) -> Option<(crate::scan_fidelity::Stats, f64)> {
    if !k.colours {
        return None;
    }
    let under = k.shown.as_ref()?.under.as_ref()?;
    let tolerance = crate::scan_fidelity::effective(app, Some(&under.scan), k.scan_tolerance);
    let wall: Vec<f32> = under.values.iter().zip(&under.cut).filter(|(_, c)| !**c).map(|(v, _)| *v).collect();
    Some((crate::scan_fidelity::stats(&wall, tolerance)?, tolerance))
}

/// The colouring of the scan over the template, as a cloud for the GPU viewport.
pub(crate) fn gpu_shade(app: &SolveApp) -> Option<solidcraft_gpu::view::GpuCloud> {
    let Some(Kind::ScanFlatTemplate(k)) = app.dialog.as_ref().map(|d| &d.kind) else { return None };
    if !k.colours {
        return None;
    }
    let (_, _, version, dots) = k.shown.as_ref()?.under.as_ref()?.dots.as_ref()?;
    Some(crate::dialogs_scan_surface::shade_cloud(SHADE_CLOUD_ID, *version, dots.clone()))
}

/// The dialog's rows.
pub fn rows(app: &SolveApp, ui: &mut egui::Ui, k: &mut FlatTemplate, inputs: &[SelInput]) -> bool {
    let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
    let sides = [tr("ui.scan_template_both"), tr("ui.scan_template_out"), tr("ui.scan_template_in")];
    choice_row(ui, tr("ui.scan_template_side"), "scan_template_side", &mut k.side, &sides);
    form_row(ui, tr("ui.scan_template_tolerance"), |ui| ui_kit::length_field(ui, &mut k.tolerance, &units, decimals));
    k.tolerance = k.tolerance.max(0.0);
    form_row(ui, tr("ui.scan_template_edge"), |ui| check(ui, &mut k.edge));
    look(app, k, inputs);
    if k.shown.as_ref().is_some_and(|s| s.under.is_some()) {
        crate::dialogs_scan_surface::colours_row(ui, &mut k.colours);
        let scan = k.shown.as_ref().and_then(|s| s.under.as_ref()).map(|u| u.scan.clone());
        k.keep_tolerance = crate::scan_fidelity::row(app, ui, scan.as_deref(), &mut k.scan_tolerance).keep;
    }
    colour(app, k);
    if k.pending.is_some() {
        scan_job::refining_note(ui, "scan_template_refining");
    }
    match k.shown.as_ref().map(|s| &s.result) {
        Some(Ok(m)) => {
            let line = crate::i18n::format(
                "ui.scan_template_found",
                &[
                    ("cutouts", &m.cutouts.len().to_string()),
                    ("area", &format!("{:.0}", m.area)),
                    ("cut", &format!("{:.0}", m.cut_area)),
                    ("tolerance", &format!("{:.1}", m.tolerance)),
                ],
            );
            ui_kit::text(ui, &line, ui_kit::Text::Note("scan_template_found"));
            let sides = m.sides.iter().map(|(_, l)| format!("{l:.1}")).collect::<Vec<_>>().join(" · ");
            let dims = crate::i18n::format("ui.scan_template_sides", &[("sides", &sides)]);
            ui_kit::text(ui, &dims, ui_kit::Text::Note("scan_template_sides"));
            if let Some((stats, tolerance)) = fidelity(app, k) {
                crate::scan_fidelity::readout(app, ui, "scan_template_fidelity", &stats, tolerance);
            }
            if m.fitted {
                let plane = crate::i18n::format("ui.scan_template_plane", &[("kept", &m.kept.to_string()), ("under", &m.under.to_string())]);
                ui_kit::text(ui, &plane, ui_kit::Text::Note("scan_template_plane"));
            }
        }
        Some(Err(why)) => {
            ui_kit::text(ui, why, ui_kit::Text::Report("scan_template_error", true));
        }
        None => {
            ui_kit::text(ui, tr("ui.scan_template_hint"), ui_kit::Text::Hint("scan_template_hint"));
        }
    }
    false
}

/// What OK runs.
pub fn commands(k: &FlatTemplate, inputs: &[SelInput]) -> Result<Vec<(String, Value)>, String> {
    let p = params(k, inputs).ok_or_else(|| tr("ui.scan_template_hint").to_owned())?;
    k.shown.as_ref().ok_or("still working out the template")?.result.as_ref().map_err(Clone::clone)?;
    let mut all = Vec::new();
    all.extend(crate::scan_fidelity::keep(k.keep_tolerance.as_ref()));
    all.push(("scan.flat_template".into(), p));
    Ok(all)
}

/// The points clicked so far, in order.
fn clicks(inputs: &[SelInput]) -> Vec<Vec3> {
    items(inputs, 0)
        .iter()
        .filter_map(|s| match s {
            Sel::Face { point, .. } => Some(*point),
            _ => None,
        })
        .collect()
}

/// A length's label at `at`.
fn label(painter: &egui::Painter, proj: &crate::viewport::Proj, at: Vec3, mm: f64) {
    let t = crate::theme::Tokens::get();
    if let Some(p) = proj.to_screen(at) {
        painter.text(p, egui::Align2::CENTER_CENTER, format!("{mm:.1}"), egui::FontId::proportional(t.font_label), t.text);
    }
}

/// The template where it will lie, over the picture: the wall's plane as a ghost, the outline with
/// the length of each side, the cut-outs and the scan's edge. Before there is a template, the
/// points clicked so far are joined and each distance told.
pub(crate) fn draw(app: &SolveApp, painter: &egui::Painter, proj: &crate::viewport::Proj) {
    let Some(d) = app.dialog.as_ref() else { return };
    let Kind::ScanFlatTemplate(k) = &d.kind else { return };
    let tokens = crate::theme::Tokens::get();
    let line = |ring: &[Vec3], looped: bool, stroke: egui::Stroke| {
        let step = (ring.len() / 20_000).max(1);
        let pts: Vec<egui::Pos2> =
            ring.iter().step_by(step).chain(looped.then(|| ring.first()).flatten()).filter_map(|q| proj.to_screen(*q)).collect();
        painter.add(egui::Shape::line(pts, stroke));
    };
    let Some(Ok(m)) = k.shown.as_ref().map(|s| &s.result) else {
        let picked = clicks(&d.inputs);
        line(&picked, false, egui::Stroke::new(1.5, tokens.accent));
        for w in picked.windows(2) {
            label(painter, proj, (w[0] + w[1]) * 0.5, w[0].dist(w[1]));
        }
        return;
    };
    let ghost: Vec<egui::Pos2> = m.ghost.iter().filter_map(|q| proj.to_screen(*q)).collect();
    if ghost.len() == 4 {
        painter.add(egui::Shape::convex_polygon(
            ghost,
            tokens.accent.gamma_multiply(0.12),
            egui::Stroke::new(1.0, tokens.accent.gamma_multiply(0.4)),
        ));
    }
    for ring in &m.edge {
        line(ring, true, egui::Stroke::new(1.0, tokens.text_dim));
    }
    for ring in &m.cutouts {
        line(ring, true, egui::Stroke::new(1.5, tokens.error));
    }
    line(&m.outline, true, egui::Stroke::new(2.0, tokens.accent));
    for (at, mm) in &m.sides {
        label(painter, proj, *at, *mm);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// A wall 400 × 300 with an outlet box standing 25 off it, as an STL.
    fn wall_stl() -> String {
        let n = 120u32;
        let z = |x: f64, y: f64| if (100.0..=160.0).contains(&x) && (100.0..=180.0).contains(&y) { 25.0 } else { 0.0 };
        let at = |i: u32, j: u32| {
            let (x, y) = (400.0 * f64::from(i) / f64::from(n), 300.0 * f64::from(j) / f64::from(n));
            [x, y, z(x, y)]
        };
        let mut out = String::from("solid s\n");
        for j in 0..n {
            for i in 0..n {
                for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    out.push_str("facet normal 0 0 0\nouter loop\n");
                    for [x, y, zz] in tri {
                        out.push_str(&format!("vertex {x} {y} {zz}\n"));
                    }
                    out.push_str("endloop\nendfacet\n");
                }
            }
        }
        out.push_str("endsolid s\n");
        out
    }

    #[test]
    fn the_outline_needs_three_clicks() {
        let k = FlatTemplate::default();
        let none: Vec<SelInput> = vec![SelInput::new("Outline Points", FACES, true).places()];
        assert!(params(&k, &none).is_none());
    }

    /// Four clicks on a wall show the template (the outlet cut out, the scan's edge) before
    /// anything changes; OK sketches it.
    #[test]
    fn clicks_on_a_wall_preview_the_template_and_ok_sketches_it() {
        let mut h = crate::scenario::Harness::new();
        let path = h.dir.join("wall.stl");
        std::fs::write(&path, wall_stl()).unwrap();
        h.app.run("file.insert_mesh", json!({"path": path})).unwrap();
        let scan = h.app.session.model.state().bodies[0].name.clone();
        h.app.run("select.clear", json!({})).unwrap();
        h.app.start("scan.flat_template");
        if let Some(d) = h.app.dialog.as_mut() {
            for (i, (x, y)) in [(20.0, 20.0), (280.0, 20.0), (280.0, 280.0), (20.0, 280.0)].into_iter().enumerate() {
                d.inputs[0].items.push(Sel::Face { body: scan.clone(), index: i, point: Vec3::new(x, y, 0.0) });
            }
        }
        h.frames(3);
        let Some(Kind::ScanFlatTemplate(k)) = h.app.dialog.as_ref().map(|d| &d.kind) else { panic!("the Flat Template dialog") };
        let m = k.shown.as_ref().and_then(|s| s.result.as_ref().ok()).expect("worked out");
        assert_eq!(m.cutouts.len(), 1, "the outlet is cut out");
        assert!(!m.edge.is_empty() && m.outline.len() == 4, "the scan's edge and the outline are shown");
        assert!(h.app.session.doc.features.iter().all(|f| f.name != "Wall Template"), "nothing sketched before OK");
        // The scan over the template is coloured by how far it stands off the plane; the wall
        // outside the outlet is flat, so all of it is within the tolerance (the outlet, a cut-out
        // by design, is not counted against it).
        let (stats, tolerance) = fidelity(&h.app, k).expect("the wall measured");
        assert!(stats.count > 100 && tolerance == 0.1 && stats.within > 0.99 && stats.rms < 0.01, "{stats:?}");
        assert!(gpu_shade(&h.app).is_some_and(|c| !c.dots.is_empty()), "the scan coloured over the template");
        h.step(&json!({"click": {"handle": "dialog:ok"}})).unwrap();
        h.settle();
        assert!(h.app.dialog.is_none(), "{:?}", h.app.dialog.as_ref().map(|d| d.error.clone()));
        assert!(h.app.session.doc.features.iter().any(|f| f.name == "Wall Template"), "the template's sketch");
    }
}
