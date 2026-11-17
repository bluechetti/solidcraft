//! The Area row the scan tools that work on part of a scan share: the whole piece clicked, the
//! face grown from the click (as Select on Scan's Region grows it), the scan round the click
//! within a radius in mm, or what is selected on the scan (brush, lasso, region, a kept
//! selection opened), fitted as it is with no click. A dialog opened with part of a scan selected
//! and nothing clicked starts on What is selected. The clean tools (Denoise, Reduce Mesh) start on
//! the whole scan instead, with no click ([`Area::clean`]).

use serde_json::{Value, json};
use solidcore_record::Vec3;

use crate::SolveApp;
use crate::dialogs::form_row;
use crate::ui_kit::{self, choice_row};

/// The ways, in the row's order.
pub const PIECE: usize = 0;
pub const GROW: usize = 1;
pub const ROUND: usize = 2;
pub const SELECTED: usize = 3;

/// The radius Round the click starts at, mm.
const RADIUS: f64 = 20.0;

/// How the area is chosen, and the radius Round the click goes to.
#[derive(Clone, Debug)]
pub struct Area {
    pub way: usize,
    /// With [`ROUND`], how far round the click, mm.
    pub radius: f64,
    /// A clean tool's area ([`Area::clean`]): its first way is the whole scan, with no click.
    pub whole: bool,
}

impl Default for Area {
    fn default() -> Self {
        Area { way: PIECE, radius: RADIUS, whole: false }
    }
}

impl Area {
    /// The area a dialog opens with: what is selected on a scan when part of one is and nothing was
    /// clicked, else the whole piece clicked.
    pub fn opening(app: &SolveApp, clicked: bool) -> Area {
        let way = if !clicked && selected_scan(app).is_some() { SELECTED } else { PIECE };
        Area { way, radius: RADIUS, whole: false }
    }

    /// A clean tool's area (Denoise, Reduce Mesh): the whole scan, as the tool always worked, until
    /// another way is chosen; a selection never narrows it unasked.
    pub fn clean() -> Area {
        Area { way: PIECE, radius: RADIUS, whole: true }
    }

    /// Whether this is a clean tool's whole scan.
    pub fn is_whole(&self) -> bool {
        self.whole && self.way == PIECE
    }

    /// What a clean tool is sent for its area, beside the scan: nothing for the whole scan,
    /// `selected` for What is selected, the click with `grow` or `radius` as the way says. `None`
    /// until there is a click, or nothing is selected.
    pub fn step_params(&self, app: &SolveApp, clicked: Option<(&str, Vec3)>) -> Option<Value> {
        if self.is_whole() {
            return Some(json!({}));
        }
        if self.way == SELECTED {
            return selected_scan(app).map(|scan| json!({ "body": scan, "selected": true }));
        }
        self.params(app, clicked)
    }

    /// As [`Area::opening`], for a tool whose own way is Round the click (`radius` mm; 0 until the
    /// scan's own is known).
    pub fn opening_round(app: &SolveApp, radius: f64) -> Area {
        let way = if selected_scan(app).is_some() { SELECTED } else { ROUND };
        Area { way, radius, whole: false }
    }

    /// The scan this area is on: what is selected on, or the scan clicked.
    pub fn scan(&self, app: &SolveApp, clicked: Option<&str>) -> Option<String> {
        if self.way == SELECTED { selected_scan(app) } else { clicked.map(str::to_owned) }
    }

    /// What the tool is sent for its region, as `scan.flatten` and `scan.fit_patches` take it: the
    /// scan clicked and the point, with `grow` or `radius` as the way says, or with What is
    /// selected the scan alone (the command takes what is selected on it). `None` until there is
    /// a click, or nothing is selected.
    pub fn params(&self, app: &SolveApp, clicked: Option<(&str, Vec3)>) -> Option<Value> {
        if self.way == SELECTED {
            return selected_scan(app).map(|scan| json!({ "body": scan }));
        }
        let (body, point) = clicked?;
        let mut p = json!({"body": body, "point": [point.x, point.y, point.z]});
        match self.way {
            GROW => p["grow"] = json!(true),
            ROUND => p["radius"] = json!(self.radius.max(1e-3)),
            _ => {}
        }
        Some(p)
    }

    /// Whether the area waits for a click (every way but What is selected).
    pub fn needs_click(&self) -> bool {
        self.way != SELECTED && !self.is_whole()
    }
}

/// The scan part of which is selected on, if any.
fn selected_scan(app: &SolveApp) -> Option<String> {
    solidcraft_engine::cmd::scan::select::selected(&app.session).filter(|k| k.count > 0).map(|k| k.scan.clone())
}

fn tr(id: &'static str) -> &'static str {
    solidcraft_ui_text::tr(id)
}

/// The Area choice, and the radius under it with Round the click. `id` names the choice's handle.
pub fn row(app: &SolveApp, ui: &mut egui::Ui, id: &str, area: &mut Area) {
    let first = if area.whole { tr("ui.scan_area_whole") } else { tr("ui.scan_area_piece") };
    let ways = [first, tr("ui.scan_area_grow"), tr("ui.scan_fit_area_round"), tr("ui.scan_area_selected")];
    choice_row(ui, tr("ui.scan_fit_area"), id, &mut area.way, &ways);
    if area.way == ROUND {
        let (units, decimals) = (app.session.doc.units.clone(), app.preferences.length_decimals);
        form_row(ui, tr("ui.scan_fit_radius"), |ui| ui_kit::length_field(ui, &mut area.radius, &units, decimals));
        area.radius = area.radius.max(0.0);
    }
}

/// What a dialog says while its area waits: click the scan, or select part of it first.
pub fn waiting(area: &Area, click: &'static str) -> &'static str {
    if area.needs_click() { tr(click) } else { tr("ui.scan_area_select_first") }
}
