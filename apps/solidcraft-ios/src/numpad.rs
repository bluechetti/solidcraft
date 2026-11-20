//! The number pad: an on-screen keypad for number and expression fields, as tablet CAD apps
//! have one. iPadOS offers no number keyboard of its own (an iPad shows the full keyboard
//! whatever layout an app asks for), so while a number field has the focus the system keyboard
//! is held back (`keyboard::set_numpad`) and this pad is drawn instead. ABC brings the system
//! keyboard back for that field, for names in an expression (`d1 * 2`).
//!
//! The pad is painted, not built from egui widgets: a widget taking the press would take the
//! focus from the field being typed in. Presses on its keys are taken out of the input
//! ([`Numpad::take_presses`]) and become the text and keys they stand for.

/// What a key does.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Action {
    Text(&'static str),
    Key(egui::Key),
    /// Hand the field to the system keyboard.
    Abc,
}

/// The keys, row by row: label, action, width in key units.
const ROWS: &[&[(&str, Action, f32)]] = &[
    &[
        ("7", Action::Text("7"), 1.0),
        ("8", Action::Text("8"), 1.0),
        ("9", Action::Text("9"), 1.0),
        ("(", Action::Text("("), 1.0),
        (")", Action::Text(")"), 1.0),
    ],
    &[
        ("4", Action::Text("4"), 1.0),
        ("5", Action::Text("5"), 1.0),
        ("6", Action::Text("6"), 1.0),
        ("×", Action::Text("*"), 1.0),
        ("÷", Action::Text("/"), 1.0),
    ],
    &[
        ("1", Action::Text("1"), 1.0),
        ("2", Action::Text("2"), 1.0),
        ("3", Action::Text("3"), 1.0),
        ("+", Action::Text("+"), 1.0),
        ("−", Action::Text("-"), 1.0),
    ],
    &[
        ("0", Action::Text("0"), 1.0),
        (".", Action::Text("."), 1.0),
        ("mm", Action::Text(" mm"), 1.0),
        ("in", Action::Text(" in"), 1.0),
        ("°", Action::Text("°"), 1.0),
    ],
    &[("ABC", Action::Abc, 1.0), ("", Action::Key(egui::Key::Backspace), 1.0), ("", Action::Key(egui::Key::Enter), 3.0)],
];
/// A key's height and the gap between keys (points).
const KEY: f32 = 52.0;
const GAP: f32 = 6.0;
const KEY_WIDTH: f32 = 64.0;
/// How long (seconds) a change must hold before the pad and keyboard follow it. While a
/// keyboard slides in the layout moves, and a field may miss its number mark for a frame or
/// two; following every frame made the keyboard flicker in and out.
const HOLD: f64 = 0.3;
/// How long (seconds) a number field keeps the focus, with nothing held down, before the pad
/// comes up.
const APPEAR: f64 = 0.25;

#[derive(Default)]
pub struct Numpad {
    /// Where each key was drawn last frame (empty while the pad is hidden).
    keys: Vec<(egui::Rect, Action)>,
    /// The pad's outline last frame.
    rect: Option<egui::Rect>,
    /// A press on a key is held until its release.
    held: Option<Action>,
    /// The field ABC handed to the system keyboard: no pad while it keeps the focus.
    abc: Option<egui::Id>,
    /// A press of ABC this frame (acted on in `show`).
    want_abc: bool,
    /// When a number field last had the focus, and when the ABC field last had it (app time).
    numeric_at: f64,
    abc_at: f64,
    /// The number field that has the focus and since when (app time): the pad comes up once it
    /// has kept it a moment with nothing held down, so a drag or a passing focus never flashes it.
    since: Option<(egui::Id, f64)>,
    /// Where the pad stands for the field it was opened for: it stays there while that field is
    /// edited, rather than following each change of the layout around it.
    placed: Option<(egui::Id, egui::Rect)>,
}

impl Numpad {
    /// Whether the pad stands in for the keyboard now.
    #[cfg(test)]
    pub fn shown(&self) -> bool {
        self.rect.is_some()
    }

    /// Turn presses on the pad's keys into the text and keys they stand for. Run on the input
    /// after the app's own sorting of touch (so pencil and finger presses both arrive as the
    /// pointer's), before egui sees it.
    pub fn take_presses(&mut self, raw: &mut egui::RawInput) {
        if self.rect.is_none() && self.held.is_none() {
            return;
        }
        let key_at = |p: egui::Pos2| self.keys.iter().find(|(r, _)| r.expand(GAP / 2.0).contains(p)).map(|(_, a)| *a);
        let on_pad = |p: egui::Pos2| self.rect.is_some_and(|r| r.contains(p));
        let mut out = Vec::with_capacity(raw.events.len());
        for event in std::mem::take(&mut raw.events) {
            match &event {
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: true, .. } if on_pad(*pos) => {
                    self.held = key_at(*pos);
                    if self.held.is_none() {
                        // Between keys: swallowed all the same, so the field keeps the focus.
                        self.held = Some(Action::Text(""));
                    }
                }
                egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed: false, modifiers } if self.held.is_some() => {
                    let pressed = self.held.take();
                    if pressed.is_some() && key_at(*pos) == pressed {
                        match pressed {
                            Some(Action::Text(t)) if !t.is_empty() => out.push(egui::Event::Text(t.to_string())),
                            Some(Action::Key(key)) => {
                                for down in [true, false] {
                                    out.push(egui::Event::Key { key, physical_key: Some(key), pressed: down, repeat: false, modifiers: *modifiers });
                                }
                            }
                            Some(Action::Abc) => self.want_abc = true,
                            _ => {}
                        }
                    }
                }
                egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } if self.held.is_some() => {}
                egui::Event::PointerGone if self.held.is_some() => {
                    self.held = None;
                }
                _ => out.push(event),
            }
        }
        raw.events = out;
    }

    /// Draw the pad while a number field (`numeric`) has the focus, inside `area` and clear of
    /// the field. Returns whether the system keyboard should stay away (always, unless ABC asked
    /// for it), and whether a number field has the focus (steadied over [`HOLD`]).
    pub fn show(&mut self, ctx: &egui::Context, area: egui::Rect, numeric: bool) -> (bool, bool) {
        let now = ctx.input(|i| i.time);
        let focused = ctx.memory(|m| m.focused());
        if numeric && focused.is_some() {
            self.numeric_at = now;
        }
        // A field that has closed (Enter, Escape, a tap elsewhere) ends the pad at once; one that
        // only missed its mark keeps it.
        let numeric = focused.is_some() && (numeric || now - self.numeric_at < HOLD);
        if self.want_abc {
            self.want_abc = false;
            self.abc = focused;
            self.abc_at = now;
        }
        if self.abc.is_some() {
            if self.abc == focused {
                self.abc_at = now;
            } else if now - self.abc_at >= HOLD {
                self.abc = None;
            }
        }
        // Steady before it shows: the same field focused for [`APPEAR`], and no touch held down
        // (unless it is on the pad itself, which is already up).
        let shown = self.rect.is_some();
        let down = ctx.input(|i| i.pointer.any_down());
        match (numeric.then_some(focused).flatten(), self.since) {
            (Some(id), Some((was, _))) if id == was => {}
            (Some(id), _) => self.since = Some((id, now)),
            (None, _) => self.since = None,
        }
        let steady = self.since.is_some_and(|(_, at)| now - at >= APPEAR) && (shown || !down);
        if !numeric || self.abc.is_some() || !steady {
            if !numeric {
                self.placed = None;
            }
            if numeric && self.abc.is_none() {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(APPEAR));
            }
            self.keys.clear();
            self.rect = None;
            // A text field (a name, a search, a formula) gets no keyboard by itself, only a small
            // ABC beside it to ask for one (or the pencil writes into it).
            let typing = focused.is_some() && ctx.egui_wants_keyboard_input();
            if !numeric && typing && self.abc.is_none() && !down {
                self.chip(ctx, area, focused);
            }
            // The system keyboard comes only when asked for (ABC).
            return (focused.is_some() && self.abc.is_none(), numeric);
        }
        let size = egui::vec2(5.0 * KEY_WIDTH + 6.0 * GAP, ROWS.len() as f32 * KEY + (ROWS.len() as f32 + 1.0) * GAP);
        // Beside what the field is in (a dialog, a value box in the view), never over it: left of
        // it, else right of it, else above or below; level with the field, kept on screen.
        let field = focused.and_then(|id| ctx.read_response(id));
        let holder = field
            .as_ref()
            .map(|r| egui::containers::AreaState::load(ctx, r.layer_id.id).map(|a| a.rect()).filter(|a| a.contains_rect(r.rect)).unwrap_or(r.rect));
        let pad = match (self.placed, focused) {
            (Some((id, rect)), Some(f)) if id == f && area.contains_rect(rect) => rect,
            _ => place(area, size, field.map(|r| r.rect), holder),
        };
        self.placed = focused.map(|f| (f, pad));
        let visuals = ctx.global_style().visuals.clone();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::unique("sc_numpad")));
        painter.rect(pad, 12.0, visuals.window_fill, visuals.window_stroke, egui::StrokeKind::Inside);
        self.keys.clear();
        let mut y = pad.top() + GAP;
        for row in ROWS {
            let mut x = pad.left() + GAP;
            for (label, action, units) in *row {
                let w = units * KEY_WIDTH + (units - 1.0) * GAP;
                let r = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, KEY));
                let held = self.held == Some(*action);
                let fill = match action {
                    Action::Key(egui::Key::Enter) => visuals.selection.bg_fill,
                    _ if held => visuals.widgets.active.bg_fill,
                    _ => visuals.widgets.inactive.weak_bg_fill,
                };
                painter.rect_filled(r, 8.0, fill);
                let ink = if matches!(action, Action::Key(egui::Key::Enter)) { visuals.selection.stroke.color } else { visuals.text_color() };
                match action {
                    // Drawn, not typeset: the app's fonts have no ⌫ or ✓.
                    Action::Key(egui::Key::Backspace) => backspace(&painter, r.center(), ink),
                    Action::Key(egui::Key::Enter) => check(&painter, r.center(), ink),
                    _ => {
                        painter.text(r.center(), egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(22.0), ink);
                    }
                }
                self.keys.push((r, *action));
                x += w + GAP;
            }
            y += KEY + GAP;
        }
        self.rect = Some(pad);
        (true, true)
    }
}

impl Numpad {
    /// The small ABC beside a focused text field: a tap gives it the system keyboard.
    fn chip(&mut self, ctx: &egui::Context, area: egui::Rect, focused: Option<egui::Id>) {
        let Some(field) = focused.and_then(|id| ctx.read_response(id)).map(|r| r.rect) else { return };
        let size = egui::vec2(KEY_WIDTH, field.height().clamp(28.0, KEY));
        let y = field.center().y - size.y / 2.0;
        let right = egui::Rect::from_min_size(egui::pos2(field.right() + 6.0, y), size);
        let left = egui::Rect::from_min_size(egui::pos2(field.left() - 6.0 - size.x, y), size);
        let Some(chip) = [right, left].into_iter().find(|r| area.contains_rect(*r)) else { return };
        let visuals = ctx.global_style().visuals.clone();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Debug, egui::Id::unique("sc_numpad_chip")));
        painter.rect(chip, 8.0, visuals.window_fill, visuals.window_stroke, egui::StrokeKind::Inside);
        let held = self.held == Some(Action::Abc);
        painter.rect_filled(chip.shrink(3.0), 6.0, if held { visuals.widgets.active.bg_fill } else { visuals.widgets.inactive.weak_bg_fill });
        painter.text(chip.center(), egui::Align2::CENTER_CENTER, "ABC", egui::FontId::proportional(15.0), visuals.text_color());
        self.keys.push((chip, Action::Abc));
        self.rect = Some(chip);
    }
}

/// Where the pad goes in `area`: beside `holder` (the window or box the `field` is in), left of
/// it first, then right, then above or below, level with the field; else the bottom right corner.
fn place(area: egui::Rect, size: egui::Vec2, field: Option<egui::Rect>, holder: Option<egui::Rect>) -> egui::Rect {
    const MARGIN: f32 = 12.0;
    let corner = egui::Rect::from_min_size(area.right_bottom() - size - egui::vec2(MARGIN, MARGIN), size);
    let (Some(field), Some(holder)) = (field, holder) else { return corner };
    let level = (field.center().y - size.y / 2.0).clamp(area.top() + MARGIN, (area.bottom() - size.y - MARGIN).max(area.top()));
    let centred = (field.center().x - size.x / 2.0).clamp(area.left() + MARGIN, (area.right() - size.x - MARGIN).max(area.left()));
    let candidates = [
        egui::pos2(holder.left() - MARGIN - size.x, level),
        egui::pos2(holder.right() + MARGIN, level),
        egui::pos2(centred, holder.bottom() + MARGIN),
        egui::pos2(centred, holder.top() - MARGIN - size.y),
    ];
    candidates.into_iter().map(|at| egui::Rect::from_min_size(at, size)).find(|r| area.contains_rect(*r)).unwrap_or(corner)
}

/// The delete key's symbol: a tag pointing left with a cross in it.
fn backspace(painter: &egui::Painter, c: egui::Pos2, ink: egui::Color32) {
    let stroke = egui::Stroke::new(2.0, ink);
    let (w, h) = (26.0, 18.0);
    let left = c.x - w / 2.0;
    let points = vec![
        egui::pos2(left, c.y),
        egui::pos2(left + 8.0, c.y - h / 2.0),
        egui::pos2(left + w, c.y - h / 2.0),
        egui::pos2(left + w, c.y + h / 2.0),
        egui::pos2(left + 8.0, c.y + h / 2.0),
    ];
    painter.add(egui::Shape::closed_line(points, stroke));
    let x = egui::pos2(c.x + 4.0, c.y);
    let d = 4.5;
    painter.line_segment([x + egui::vec2(-d, -d), x + egui::vec2(d, d)], stroke);
    painter.line_segment([x + egui::vec2(-d, d), x + egui::vec2(d, -d)], stroke);
}

/// The confirm key's symbol: a tick.
fn check(painter: &egui::Painter, c: egui::Pos2, ink: egui::Color32) {
    let points = vec![c + egui::vec2(-11.0, 0.0), c + egui::vec2(-3.0, 8.0), c + egui::vec2(12.0, -9.0)];
    painter.add(egui::Shape::line(points, egui::Stroke::new(3.0, ink)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(at: egui::Pos2, down: bool) -> egui::Event {
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: down, modifiers: egui::Modifiers::NONE }
    }

    fn pad() -> (Numpad, egui::Context) {
        let ctx = egui::Context::default();
        let mut pad = Numpad::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.memory_mut(|m| m.request_focus(egui::Id::unique("field")));
        });
        out.textures_delta.clear();
        // The field keeps the focus past [`APPEAR`]: the pad comes up.
        for time in [0.0, APPEAR + 0.05] {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| {
                ui.memory_mut(|m| m.request_focus(egui::Id::unique("field")));
                pad.show(ui.ctx(), egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0)), true);
            });
            out.textures_delta.clear();
        }
        (pad, ctx)
    }

    #[test]
    fn a_field_focused_during_a_drag_brings_no_pad_until_the_touch_lifts() {
        let ctx = egui::Context::default();
        let mut pad = Numpad::default();
        let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0));
        let frame = |pad: &mut Numpad, time: f64, events: Vec<egui::Event>| {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(time), events, ..Default::default() }, |ui| {
                ui.memory_mut(|m| m.request_focus(egui::Id::unique("field")));
                pad.show(ui.ctx(), area, true);
            });
            out.textures_delta.clear();
        };
        let at = egui::pos2(500.0, 400.0);
        frame(&mut pad, 0.0, vec![egui::Event::PointerMoved(at), press(at, true)]);
        assert!(!pad.shown(), "not at once");
        frame(&mut pad, 1.0, vec![egui::Event::PointerMoved(egui::pos2(520.0, 400.0))]);
        assert!(!pad.shown(), "not while the drag holds the touch down");
        frame(&mut pad, 1.1, vec![press(egui::pos2(520.0, 400.0), false)]);
        frame(&mut pad, 1.2, Vec::new());
        assert!(pad.shown(), "once the touch has lifted");
    }

    fn key(pad: &Numpad, label_action: Action) -> egui::Pos2 {
        pad.keys.iter().find(|(_, a)| *a == label_action).map(|(r, _)| r.center()).unwrap_or_default()
    }

    #[test]
    fn a_tap_on_a_key_types_it_and_reaches_nothing_else() {
        let (mut p, _ctx) = pad();
        assert!(p.shown());
        let at = key(&p, Action::Text("7"));
        let mut raw = egui::RawInput { events: vec![egui::Event::PointerMoved(at), press(at, true)], ..Default::default() };
        p.take_presses(&mut raw);
        assert!(raw.events.iter().all(|e| !matches!(e, egui::Event::PointerButton { .. })), "the press never reaches the field");
        let mut raw = egui::RawInput { events: vec![press(at, false), egui::Event::PointerGone], ..Default::default() };
        p.take_presses(&mut raw);
        // The finger leaving afterwards may pass: it presses nothing.
        assert_eq!(raw.events.first(), Some(&egui::Event::Text("7".into())));
        assert!(raw.events.iter().all(|e| !matches!(e, egui::Event::PointerButton { .. })));
    }

    #[test]
    fn enter_and_backspace_are_keys() {
        let (mut p, _ctx) = pad();
        let at = key(&p, Action::Key(egui::Key::Enter));
        let mut raw = egui::RawInput { events: vec![press(at, true), press(at, false)], ..Default::default() };
        p.take_presses(&mut raw);
        assert!(raw.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::Enter, pressed: true, .. })));
    }

    #[test]
    fn a_press_off_the_pad_passes_and_no_number_field_hides_it() {
        let (mut p, ctx) = pad();
        let off = egui::pos2(5.0, 5.0);
        let mut raw = egui::RawInput { events: vec![press(off, true)], ..Default::default() };
        p.take_presses(&mut raw);
        assert_eq!(raw.events.len(), 1);
        // Held a moment through a missed mark, then gone.
        let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1000.0, 800.0));
        let field = egui::Id::unique("field");
        let mut out = ctx.run_ui(egui::RawInput { time: Some(0.1), ..Default::default() }, |ui| {
            ui.memory_mut(|m| m.request_focus(field));
            assert!(p.show(ui.ctx(), area, false).0, "one frame without the mark keeps the pad");
        });
        out.textures_delta.clear();
        let mut out = ctx.run_ui(egui::RawInput { time: Some(1.0), ..Default::default() }, |ui| {
            ui.memory_mut(|m| m.request_focus(field));
            let (keyboard_away, numeric) = p.show(ui.ctx(), area, false);
            assert!(!numeric, "a field that stays without the mark loses the pad");
            assert!(keyboard_away, "and still gets no system keyboard until ABC asks for it");
        });
        out.textures_delta.clear();
        assert!(!p.shown());
    }
}
