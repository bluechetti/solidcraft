//! Export Panel's drawings as pages of lines and one-line labels, written as one PDF through the
//! sheet's paper scene ([`solidcraft_geom::paper`]) and PDF writer ([`solidcraft_io::paper`]). A
//! label is set in the built-in font by [`solidcraft_sketch::paper_text`], anchored at its start,
//! middle or end.

use solidcore_record::{Seg2, Vec2};
use solidcraft_geom::paper::{self, Group, ObjectKind, Scene};
use solidcraft_sketch::{TextAlign, TextStyle};

use crate::{EngineError, Result};

/// Where a label's line sits on its point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Anchor {
    /// The line starts at the point.
    Start,
    /// The line is centred on it.
    Middle,
    /// The line ends at it.
    End,
}

/// One mark on a page.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Item {
    /// A solid line `width_mm` wide along `path`; a closed stroke ends where it starts.
    Stroke { path: Vec<Seg2>, closed: bool, width_mm: f64 },
    /// One line of text, its capital height `height_mm`, its baseline turned `angle` radians about `at`.
    Text { at: Vec2, height_mm: f64, angle: f64, anchor: Anchor, text: String },
}

/// One page: the drawing's lines and its words and dimensions, each painted as one group.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Page {
    pub size_mm: Vec2,
    pub view: Vec<Item>,
    pub notes: Vec<Item>,
}

/// The language labels are set in.
const LANGUAGE: &str = "en";

impl Page {
    /// The page as a sheet's paper scene: the view, then the notes.
    fn scene(&self) -> Result<Scene> {
        let mut scene = Scene { size_mm: self.size_mm, groups: Vec::new(), fonts: Vec::new(), clips: Vec::new() };
        for (object, kind, items) in [(1, ObjectKind::View, &self.view), (2, ObjectKind::Note, &self.notes)] {
            let mut marks = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Item::Stroke { path, closed, width_mm } => {
                        marks.push(paper::Item::Stroke { path: path.clone(), closed: *closed, width_mm: *width_mm, pattern: None });
                    }
                    Item::Text { at, height_mm, angle, anchor, text } => {
                        let align = match anchor {
                            Anchor::Start => TextAlign::Left,
                            Anchor::Middle => TextAlign::Center,
                            Anchor::End => TextAlign::Right,
                        };
                        let style = TextStyle { align, ..TextStyle::default() };
                        let set = solidcraft_sketch::paper_text(text, None, LANGUAGE, *at, *height_mm, *angle, &style)
                            .map_err(|e| EngineError::Other(format!("a drawing label: {e}")))?;
                        if let Some((runs, fonts)) = set {
                            marks.extend(runs.into_iter().map(paper::Item::Text));
                            scene.add_fonts(fonts);
                        }
                    }
                }
            }
            scene.groups.push(Group { object, kind, items: marks });
        }
        Ok(scene)
    }
}

/// The pages as one PDF document, a page each, in order.
pub(super) fn pdf(pages: &[Page]) -> Result<Vec<u8>> {
    let scenes = pages.iter().map(Page::scene).collect::<Result<Vec<_>>>()?;
    let prepared = scenes.iter().map(|s| s.prepare()).collect::<std::result::Result<Vec<_>, _>>().map_err(|e| EngineError::Other(e.to_string()))?;
    let (bytes, _) = solidcraft_io::paper::paper_pdf_pages(&prepared, &solidcraft_io::paper::PdfOptions::default())?;
    Ok(bytes)
}
