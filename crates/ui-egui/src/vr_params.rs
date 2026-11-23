//! The Parameters as the headset shows them (docs/vr.md): every parameter of the design in a list
//! made for a controller's ray, in the window on the belt. A parameter whose expression is a plain
//! number has big − and + steps either side of its value (a unit a step, a tenth with Fine on); any
//! parameter takes an exact value from the keypad. A formula is never stepped: only the keypad
//! replaces it, on purpose. Each change goes through `parameters.change`, as the window's dialog
//! sends it: one undo step, and the design follows at once, on the desktop too.

use serde_json::json;

use crate::SolveApp;
use crate::params_dialog::{self, Row};

/// The list between frames: the rows as listed at one revision of the design (listing evaluates
/// every parameter, so it happens when the design changes, not every frame), the value being
/// typed on the keypad, whether steps are fine, and the last refusal.
#[derive(Default)]
pub struct VrParams {
    listed: Option<(u64, Vec<Row>)>,
    /// The parameter the keypad types for, and what has been typed.
    typing: Option<(String, String)>,
    /// Steps a tenth of a unit (a tenth of 5° for angles) instead of a whole one.
    pub fine: bool,
    /// Why the last change was refused, in the engine's words.
    error: Option<String>,
}

/// A keypad key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Digit(u8),
    Point,
    /// Changes the sign of what is typed.
    Minus,
    Back,
}

/// The keys, row by row, as the keypad lays them out.
pub const KEYPAD: [[Key; 3]; 4] = [
    [Key::Digit(7), Key::Digit(8), Key::Digit(9)],
    [Key::Digit(4), Key::Digit(5), Key::Digit(6)],
    [Key::Digit(1), Key::Digit(2), Key::Digit(3)],
    [Key::Minus, Key::Digit(0), Key::Point],
];

/// The most characters the keypad takes.
const MAX_TYPED: usize = 16;

/// `typed` after `key`: one point at most, the sign kept in front, nothing past [`MAX_TYPED`].
pub fn press(typed: &str, key: Key) -> String {
    let mut out = typed.to_string();
    match key {
        Key::Digit(d) if out.len() < MAX_TYPED && d < 10 => out.push(char::from(b'0' + d)),
        Key::Point if out.len() < MAX_TYPED && !out.contains('.') => {
            if out.is_empty() || out == "-" {
                out.push('0');
            }
            out.push('.');
        }
        Key::Minus => {
            if let Some(rest) = out.strip_prefix('-') {
                out = rest.to_string();
            } else {
                out.insert(0, '-');
            }
        }
        Key::Back => {
            out.pop();
        }
        _ => {}
    }
    out
}

/// The expression for what the keypad typed, in the parameter's own unit; none while it is not
/// yet a number (empty, a lone sign or point).
pub fn typed_expression(typed: &str, unit: &str) -> Option<String> {
    let v: f64 = typed.parse().ok().filter(|v: &f64| v.is_finite())?;
    Some(with_unit(v, unit))
}

/// The number an expression is, when it is only a number with or without the parameter's unit
/// (`12`, `12.5 mm`, `-3mm`); a formula, a name or another unit is none, and is never stepped.
pub fn plain_number(expression: &str, unit: &str) -> Option<f64> {
    let e = expression.trim();
    let number = if unit.is_empty() { e } else { e.strip_suffix(unit).map_or(e, str::trim_end) };
    number.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// How far one step goes in `unit`: a unit (5° for angles, a hundredth of a metre, a sixteenth
/// of an inch), a tenth of it with `fine`. A text parameter has none.
pub fn step_of(unit: &str, fine: bool) -> Option<f64> {
    let whole = match unit {
        "text" => return None,
        "deg" => 5.0,
        "rad" => 0.1,
        "m" => 0.01,
        "in" => 0.0625,
        "ft" => 0.1,
        "um" => 10.0,
        _ => 1.0,
    };
    Some(if fine { whole / 10.0 } else { whole })
}

/// The expression one step `up` (or down) from `row`'s, when it is a plain number.
pub fn stepped(expression: &str, unit: &str, up: bool, fine: bool) -> Option<String> {
    let v = plain_number(expression, unit)?;
    let step = step_of(unit, fine)?;
    let next = if up { v + step } else { v - step };
    // Rounded to the step's own places, so 0.1 steps never show 0.30000000000000004.
    let places = (0..=9).find(|&p| {
        let scaled = step * 10f64.powi(p);
        (scaled - scaled.round()).abs() < 1e-9
    });
    let scale = 10f64.powi(places.unwrap_or(9));
    Some(with_unit((next * scale).round() / scale, unit))
}

fn with_unit(v: f64, unit: &str) -> String {
    let v = if v == 0.0 { 0.0 } else { v };
    let number = format!("{v}");
    if unit.is_empty() || unit == "text" { number } else { format!("{number} {unit}") }
}

/// What a row shows as its value.
fn value_words(r: &Row) -> String {
    match (r.value, r.text.as_ref()) {
        (_, Some(text)) => text.clone(),
        (Some(v), None) if r.unit.is_empty() => format!("{}", (v * 1e4).round() / 1e4),
        (Some(v), None) => format!("{} {}", (v * 1e4).round() / 1e4, r.unit),
        (None, None) => "—".into(),
    }
}

/// How high (points) a row and a keypad key are: a target the ray hits from arm's length.
const ROW: f32 = 34.0;

/// The list into `ui` (of the headset's context), between the window's frames.
pub fn show(app: &mut SolveApp, ui: &mut egui::Ui, s: &mut VrParams) {
    let revision = app.session.revision;
    if s.listed.as_ref().is_none_or(|(r, _)| *r != revision) {
        s.listed = Some((revision, params_dialog::rows(app)));
    }
    let mut rows = s.listed.as_ref().map(|(_, rows)| rows.clone()).unwrap_or_default();
    // The user's own parameters first, then the model's, feature by feature, as the dialog.
    rows.sort_by(|a, b| (!a.group.is_empty(), &a.group).cmp(&(!b.group.is_empty(), &b.group)));
    let t = solidcraft_theme::Tokens::get();
    let big = egui::FontId::proportional(t.font_label * 1.25);

    ui.horizontal(|ui| {
        ui.checkbox(&mut s.fine, solidcraft_ui_text::tr("vr.params_fine"));
        if let Some(error) = &s.error {
            ui.colored_label(t.error, error);
        }
    });

    // The keypad, above the list while a value is typed.
    if let Some((name, typed)) = s.typing.clone() {
        let unit = rows.iter().find(|r| r.name == name).map(|r| r.unit.clone()).unwrap_or_default();
        let mut next = typed.clone();
        let mut done = None;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(egui::RichText::new(format!("{name} = {} {unit}", if typed.is_empty() { "…" } else { &typed })).font(big.clone()));
            for keys in KEYPAD {
                ui.horizontal(|ui| {
                    for key in keys {
                        let words = match key {
                            Key::Digit(d) => d.to_string(),
                            Key::Point => ".".into(),
                            Key::Minus => "±".into(),
                            Key::Back => "⌫".into(),
                        };
                        if ui.add_sized([ROW * 1.6, ROW], egui::Button::new(egui::RichText::new(words).font(big.clone()))).clicked() {
                            next = press(&next, key);
                        }
                    }
                });
            }
            ui.horizontal(|ui| {
                if ui.add_sized([ROW * 1.6, ROW], egui::Button::new(egui::RichText::new("⌫").font(big.clone()))).clicked() {
                    next = press(&next, Key::Back);
                }
                let ok = typed_expression(&next, &unit);
                if ui
                    .add_enabled(ok.is_some(), egui::Button::new(solidcraft_ui_text::tr("vr.params_set")).min_size(egui::vec2(ROW * 1.6, ROW)))
                    .clicked()
                {
                    done = Some(ok);
                }
                if ui.add_sized([ROW * 1.6, ROW], egui::Button::new(solidcraft_ui_text::tr("vr.params_cancel"))).clicked() {
                    done = Some(None);
                }
            });
        });
        match done {
            Some(Some(expression)) => {
                change(app, s, &name, &expression);
                s.typing = None;
            }
            Some(None) => s.typing = None,
            None => s.typing = Some((name, next)),
        }
    }

    let mut todo: Option<(String, String)> = None;
    let mut type_for: Option<String> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let mut group: Option<&str> = None;
        for r in &rows {
            if group != Some(r.group.as_str()) {
                group = Some(r.group.as_str());
                let heading = if r.group.is_empty() { solidcraft_ui_text::tr("vr.params_user").to_string() } else { r.group.clone() };
                ui.add_space(t.space_4);
                ui.label(egui::RichText::new(heading).strong());
            }
            ui.horizontal(|ui| {
                ui.set_min_height(ROW);
                let name_w = ui.available_width() * 0.38;
                ui.add_sized([name_w, ROW], egui::Label::new(egui::RichText::new(&r.name).font(big.clone())).truncate());
                let down = stepped(&r.expression, &r.unit, false, s.fine);
                if ui
                    .add_enabled(down.is_some(), egui::Button::new(egui::RichText::new("−").font(big.clone())).min_size(egui::vec2(ROW * 1.3, ROW)))
                    .clicked()
                    && let Some(e) = down
                {
                    todo = Some((r.name.clone(), e));
                }
                // The + at the right edge, the value filling what is left, cut short to fit: a long
                // formula never pushes the + off the window.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let up = stepped(&r.expression, &r.unit, true, s.fine);
                    if ui
                        .add_enabled(up.is_some(), egui::Button::new(egui::RichText::new("+").font(big.clone())).min_size(egui::vec2(ROW * 1.3, ROW)))
                        .clicked()
                        && let Some(e) = up
                    {
                        todo = Some((r.name.clone(), e));
                    }
                    let value = value_words(r);
                    let shown = if plain_number(&r.expression, &r.unit).is_some() || r.text.is_some() {
                        value
                    } else {
                        format!("{value}  ({})", r.expression)
                    };
                    let colour = if r.error.is_some() { t.error } else { ui.visuals().text_color() };
                    let button = egui::Button::new(egui::RichText::new(shown).font(big.clone()).color(colour))
                        .frame(false)
                        .wrap_mode(egui::TextWrapMode::Truncate);
                    if ui.add_sized([ui.available_width(), ROW], button).clicked() && r.text.is_none() {
                        type_for = Some(r.name.clone());
                    }
                });
            });
        }
        if rows.is_empty() {
            ui.label(solidcraft_ui_text::tr("vr.params_none"));
        }
    });
    if let Some(name) = type_for {
        s.typing = Some((name, String::new()));
    }
    if let Some((name, expression)) = todo {
        change(app, s, &name, &expression);
    }
}

fn change(app: &mut SolveApp, s: &mut VrParams, name: &str, expression: &str) {
    s.error = app.run_heavy("parameters.change", json!({"name": name, "expression": expression})).err();
}
