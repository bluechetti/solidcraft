//! Tags over the view (the labels Scan Deviation, Hole Table and Measure Checks pin to points of
//! the model) laid out so that none covers another: each starts above and to the right of its
//! point and moves down past any tag already placed that it would cover. The leader still runs
//! from the point.

use egui::{Pos2, Rect, Vec2, vec2};

/// The tags placed so far in one drawing.
#[derive(Default)]
pub(crate) struct Tags {
    placed: Vec<Rect>,
}

/// The gap kept between two tags, in points.
const GAP: f32 = 3.0;

impl Tags {
    /// Where a tag of `size` for the point `at` goes: its usual place, or below the tags it would
    /// cover there.
    pub(crate) fn place(&mut self, at: Pos2, size: Vec2) -> Rect {
        let mut r = Rect::from_min_size(at + vec2(10.0, -10.0 - size.y), size);
        // Each move clears one tag, so at most as many moves as tags.
        for _ in 0..=self.placed.len() {
            let Some(below) = self.placed.iter().filter(|p| p.expand(GAP).intersects(r)).map(|p| p.max.y).reduce(f32::max) else { break };
            r = r.translate(vec2(0.0, below + GAP - r.min.y));
        }
        self.placed.push(r);
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_at_one_point_stack_and_apart_ones_stay() {
        let mut tags = Tags::default();
        let size = vec2(120.0, 20.0);
        let a = tags.place(Pos2::new(100.0, 100.0), size);
        let b = tags.place(Pos2::new(100.0, 100.0), size);
        let c = tags.place(Pos2::new(130.0, 102.0), size);
        assert!(!a.intersects(b) && !a.intersects(c) && !b.intersects(c), "{a:?} {b:?} {c:?}");
        assert!(b.min.y >= a.max.y && c.min.y >= b.max.y, "stacked downward");
        let far = tags.place(Pos2::new(600.0, 400.0), size);
        assert_eq!(far, Rect::from_min_size(Pos2::new(610.0, 370.0), size), "a tag apart keeps its usual place");
    }
}
