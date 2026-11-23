//! The Assistant in the VR view (docs/vr.md): the desktop's Assistant (the agent chat, its
//! conversation, Allow and Deny, Undo changes) drawn into the window beside the belt, with Talk on
//! top: held, it runs the voice program the host names (`SOLIDCRAFT_VR_VOICE`: it records until
//! its input closes and prints what it heard); let go, what it heard is sent as the next message.
//! SolidCraft itself records and transcribes nothing; with no voice program, Talk says so.

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{Receiver, channel};

use crate::SolveApp;

/// What Talk is doing between frames.
#[derive(Default)]
pub struct VrAssistant {
    #[cfg(not(target_arch = "wasm32"))]
    listening: Option<std::process::Child>,
    #[cfg(not(target_arch = "wasm32"))]
    hearing: Option<Receiver<Result<String, String>>>,
    /// What the line under Talk says: what was heard, or why nothing was.
    pub note: String,
    /// The undo history's length when the last request was sent: once the agent is done, a design
    /// changed since is offered to keep or undo (the change seen before it is kept).
    base: Option<usize>,
    review: bool,
}

/// The environment variable that names the voice program.
pub const VOICE: &str = "SOLIDCRAFT_VR_VOICE";

/// What was heard, as it is sent: one line, trimmed; nothing when only blanks or whisper's
/// markers for silence (`[BLANK_AUDIO]`) were heard.
pub fn heard(out: &str) -> Option<String> {
    let line = out.split_whitespace().filter(|w| !(w.starts_with('[') && w.ends_with(']'))).collect::<Vec<_>>().join(" ");
    (!line.is_empty()).then_some(line)
}

impl VrAssistant {
    pub fn listening(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        return self.listening.is_some();
        #[cfg(target_arch = "wasm32")]
        false
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn start(&mut self) {
        let Some(program) = std::env::var_os(VOICE).filter(|p| !p.is_empty()) else {
            self.note = solidcraft_ui_text::tr("vr.talk_no_voice").to_string();
            return;
        };
        match std::process::Command::new(program)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(child) => {
                self.listening = Some(child);
                self.note = solidcraft_ui_text::tr("vr.talk_listening").to_string();
            }
            Err(e) => self.note = e.to_string(),
        }
    }

    /// Talk let go: the program's input closes and what it heard comes back off the window thread.
    #[cfg(not(target_arch = "wasm32"))]
    fn stop(&mut self) {
        let Some(mut child) = self.listening.take() else { return };
        drop(child.stdin.take());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let reply = child.wait_with_output().map_err(|e| e.to_string()).and_then(|out| {
                if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(format!("voice: {}", out.status)) }
            });
            let _ = tx.send(reply);
        });
        self.hearing = Some(rx);
        self.note = solidcraft_ui_text::tr("vr.talk_hearing").to_string();
    }

    /// What was heard, once it is in: sent as the next message.
    #[cfg(not(target_arch = "wasm32"))]
    fn poll(&mut self, app: &mut SolveApp) {
        let Some(rx) = &self.hearing else { return };
        let Ok(reply) = rx.try_recv() else { return };
        self.hearing = None;
        match reply.map(|out| heard(&out)) {
            Ok(Some(text)) => {
                self.note = format!("“{text}”");
                self.base = Some(app.session.undo.len());
                self.review = false;
                crate::agent_chat::send_text(app, text);
            }
            Ok(None) => self.note = solidcraft_ui_text::tr("vr.talk_nothing").to_string(),
            Err(e) => self.note = e,
        }
    }
}

/// The Assistant into `ui` (of the headset's context), between the window's frames.
pub fn show(app: &mut SolveApp, ui: &mut egui::Ui, s: &mut VrAssistant) {
    if !crate::agent_chat::shown(app) {
        ui.label(solidcraft_ui_text::tr("vr.assistant_off"));
        return;
    }
    app.agent_chat.open = true;
    let t = solidcraft_theme::Tokens::get();
    let words = if s.listening() { solidcraft_ui_text::tr("vr.talk_let_go") } else { solidcraft_ui_text::tr("vr.talk") };
    let size = egui::vec2(ui.available_width(), 40.0);
    let talk = ui.add_sized(size, egui::Button::new(egui::RichText::new(words).size(t.font_label * 1.3)).selected(s.listening()));
    #[cfg(not(target_arch = "wasm32"))]
    {
        let held = talk.is_pointer_button_down_on();
        if held && !s.listening() && s.hearing.is_none() {
            s.start();
        } else if !held && s.listening() {
            s.stop();
        }
        s.poll(app);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = talk;
    if !s.note.is_empty() {
        ui.label(egui::RichText::new(&s.note).color(t.text_dim));
    }
    // Once the agent is done with a request that changed the design: Keep it, or Undo it back to
    // where the request found it.
    let working = matches!(app.agent_chat.state(), crate::agent_chat::State::Busy | crate::agent_chat::State::Starting);
    if let Some(base) = s.base
        && !working
        && !s.listening()
    {
        if app.session.undo.len() > base {
            s.review = true;
        } else {
            s.base = None;
        }
    }
    if s.review
        && let Some(base) = s.base
    {
        ui.label(egui::RichText::new(solidcraft_ui_text::tr("vr.assistant_changed")).strong());
        ui.horizontal(|ui| {
            let half = egui::vec2((ui.available_width() - t.space_8) / 2.0, 36.0);
            if ui.add_sized(half, egui::Button::new(solidcraft_ui_text::tr("vr.assistant_keep"))).clicked() {
                s.review = false;
                s.base = None;
                s.note = solidcraft_ui_text::tr("vr.assistant_kept").to_string();
            }
            if ui.add_sized(half, egui::Button::new(solidcraft_ui_text::tr("vr.assistant_undo"))).clicked() {
                while app.session.undo.len() > base {
                    if app.run("edit.undo", serde_json::json!({})).is_err() {
                        break;
                    }
                }
                s.review = false;
                s.base = None;
                s.note = solidcraft_ui_text::tr("vr.assistant_undone").to_string();
            }
        });
    }
    ui.separator();
    crate::agent_chat::shared_content(app, ui, solidcraft_engine::panels::Flow::Tall);
}

/// The headset's Assistant closed: a recording going on stops, what it hears is dropped.
pub fn close(s: &mut VrAssistant) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(mut child) = s.listening.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        s.hearing = None;
    }
}
