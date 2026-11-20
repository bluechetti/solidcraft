//! Export Panel's drawings as pages of lines and one-line labels, written as one PDF through the
//! sheet's paper scene ([`solidcraft_geom::paper`]) and PDF writer ([`solidcraft_io::paper`]). A
//! label is set in the built-in font by [`solidcraft_sketch::paper_text`], anchored at its start,
//! middle or end, and drawn as its letters' outlines.

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
/// How wide the line round each letter is drawn.
const LETTER_LINE_MM: f64 = 0.1;

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
                        // Drawn as its letters' outlines: the sheet's PDF writer passes text runs
                        // over for now, and a cutting drawing without its numbers is no use.
                        if let Some((runs, fonts)) = set {
                            for run in &runs {
                                let Some(font) = fonts.iter().find(|f| f.id == run.font) else { continue };
                                marks.extend(run.outlines(font).map(|l| paper::Item::Stroke {
                                    path: l.segs,
                                    closed: true,
                                    width_mm: LETTER_LINE_MM,
                                    pattern: None,
                                }));
                            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A label reaches the page as closed lines round its letters, not as a text run: the sheet's
    /// PDF writer passes text runs over, and the drawing's numbers must be on the paper.
    #[test]
    fn a_label_is_drawn_as_its_letters_outlines() {
        let label = Item::Text { at: Vec2::new(20.0, 30.0), height_mm: 5.0, angle: 0.0, anchor: Anchor::Start, text: "P1".into() };
        let page = Page { size_mm: Vec2::new(100.0, 60.0), view: vec![label], notes: Vec::new() };
        let scene = page.scene().unwrap();
        let marks = &scene.groups[0].items;
        assert!(marks.len() >= 3, "P has two outlines and 1 has one: {}", marks.len());
        assert!(marks.iter().all(|m| matches!(m, paper::Item::Stroke { closed: true, .. })), "{marks:?}");
        // The letters' straight sides stand on the baseline at the label's point, a capital 5 mm tall.
        let ys: Vec<f64> = marks
            .iter()
            .flat_map(|m| match m {
                paper::Item::Stroke { path, .. } => {
                    path.iter().filter_map(|s| if let Seg2::Line { a, .. } = s { Some(a.y) } else { None }).collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .collect();
        let (low, high) = ys.iter().fold((f64::MAX, f64::MIN), |(l, h), y| (l.min(*y), h.max(*y)));
        assert!((low - 30.0).abs() < 0.2 && (high - 35.0).abs() < 0.2, "{low} to {high}");
        assert!(pdf(&[page]).unwrap().starts_with(b"%PDF-"));
    }
}
