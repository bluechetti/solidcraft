//! What UIKit's touches are, for upstream's Pencil mode ([`solidcraft_ui_egui::pencil`]).
//!
//! winit hands every touch to egui as `Event::Touch`; on an iPad only the Apple Pencil reports a
//! force, so a touch with one is the pencil's. This tells the app where the pencil was pressed and
//! lifted, which touches are the pencil's, and where a finger went down: what the web app learns
//! from the browser's pointer events.
use egui::{Event, Pos2, TouchId, TouchPhase};
use solidcraft_ui_egui::pencil::PlatformTaps;

#[derive(Default)]
pub struct Touches {
    /// The pencil touch in progress.
    pen: Option<TouchId>,
}

impl Touches {
    pub fn taps(&mut self, raw: &egui::RawInput) -> PlatformTaps {
        let mut taps = PlatformTaps::default();
        for event in &raw.events {
            let Event::Touch { id, phase, pos, force, .. } = event else { continue };
            let pen = force.is_some() || self.pen == Some(*id);
            if pen {
                taps.stylus.push(*id);
                match phase {
                    TouchPhase::Start => {
                        self.pen = Some(*id);
                        taps.pen_press = Some(*pos);
                    }
                    TouchPhase::End => {
                        self.pen = None;
                        taps.pen_release = Some(*pos);
                    }
                    TouchPhase::Cancel => self.pen = None,
                    TouchPhase::Move => {}
                }
            } else if *phase == TouchPhase::Start {
                taps.finger_down = Some(Pos2::new(pos.x, pos.y));
            }
        }
        taps.pen_down = self.pen.is_some();
        taps
    }
}
