//! The panel's pieces and insert pieces at full size, tiled over sheets of paper to tape together
//! and onto the board: each piece's outline and cut-outs (the cutting line), its holes and fixing
//! points (a circle at the drill size with a centre cross), at 1:1. A page is printed only where
//! there is something to cut or drill; a piece over more than one page gets a map of its pages
//! first. Each page has a trim border, its name and neighbours, marks every 100 mm in the piece's
//! own coordinates (so a page can also be placed by measuring from the piece's 0, 0) and a
//! 100 mm bar to check the print's scale.

use solidcore_record::{Seg2, Vec2};
use solidcraft_geom::paper::{Anchor, Group, Item, ObjectKind, Scene};
use solidcraft_geom::prelude::*;

use super::panel;
use super::{hole_size, wrap};

/// Paper kept clear round each page's tile (mm): the printer's edge, the name and the scale bar.
const MARGIN: f64 = 12.0;
/// Line widths (mm): the cutting line, holes, centre crosses and 100 mm marks, the trim border.
const CUT: f64 = 0.5;
const HOLE: f64 = 0.3;
const FINE: f64 = 0.12;
const TRIM: f64 = 0.15;
/// The most pages the print may have, whatever the pieces.
const MOST_PAGES: usize = 3000;

/// A piece to print: its name ("Piece 3"), its short name for page names ("P3"), how to cut it.
pub(super) struct Part<'a> {
    pub name: String,
    pub short: String,
    pub piece: &'a panel::Piece,
    pub note: Option<String>,
}

/// Letter or A4, landscape (mm); `None` for no print.
pub(super) fn paper_of(name: Option<&str>) -> Option<Result<[f64; 2], ()>> {
    match name.map(str::trim) {
        None | Some("") | Some("letter") => Some(Ok([279.4, 215.9])),
        Some("a4") => Some(Ok([297.0, 210.0])),
        Some("none") => None,
        Some(_) => Some(Err(())),
    }
}

/// The part's lines in its own coordinates: what is cut and what is drilled (with its label).
struct Drawn {
    cut: Vec<(Vec2, Vec2)>,
    marks: Vec<(Vec2, Vec2, f64)>,
    labels: Vec<(Vec2, String)>,
}

fn drawn(part: &Part, sizes: &[f64], fixing: f64) -> Drawn {
    let mut cut = Vec::new();
    for ring in std::iter::once(&part.piece.outline).chain(&part.piece.inside) {
        for (k, a) in ring.iter().enumerate() {
            let b = ring[(k + 1) % ring.len()];
            if a.dist(b) > 1e-6 {
                cut.push((*a, b));
            }
        }
    }
    let (mut marks, mut labels) = (Vec::new(), Vec::new());
    for pt in &part.piece.points {
        let (d, label) = match pt.kind {
            panel::PointKind::Hole => (hole_size(pt.diameter, sizes), pt.number.to_string()),
            panel::PointKind::Fixing => (fixing, format!("F{}", pt.number)),
        };
        let r = (d / 2.0).clamp(0.5, 200.0);
        const SIDES: usize = 48;
        for k in 0..SIDES {
            let t = |k: usize| std::f64::consts::TAU * k as f64 / SIDES as f64;
            let on = |t: f64| pt.at + Vec2::new(t.cos(), t.sin()) * r;
            marks.push((on(t(k)), on(t(k + 1)), HOLE));
        }
        let arm = r + 4.0;
        marks.push((pt.at - Vec2::new(arm, 0.0), pt.at + Vec2::new(arm, 0.0), FINE));
        marks.push((pt.at - Vec2::new(0.0, arm), pt.at + Vec2::new(0.0, arm), FINE));
        labels.push((pt.at + Vec2::new(r + 1.5, r + 1.5), format!("{label}  drill {d:.1}")));
    }
    Drawn { cut, marks, labels }
}

/// The part of the line `a`–`b` inside the box `lo`–`hi` (Liang–Barsky), if any.
fn clip(a: Vec2, b: Vec2, lo: Vec2, hi: Vec2) -> Option<(Vec2, Vec2)> {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d.x, a.x - lo.x), (d.x, hi.x - a.x), (-d.y, a.y - lo.y), (d.y, hi.y - a.y)] {
        if p.abs() < 1e-12 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t1 - t0 > 1e-9).then(|| (a + d * t0, a + d * t1))
}

fn stroke(a: Vec2, b: Vec2, width_mm: f64) -> Item {
    Item::Stroke { path: vec![Seg2::Line { a, b }], closed: false, width_mm }
}

fn text(at: Vec2, height_mm: f64, anchor: Anchor, text: impl Into<String>) -> Item {
    let mut text: String = text.into();
    text.retain(|c| !c.is_control());
    text.truncate(300);
    if text.is_empty() {
        text.push(' ');
    }
    Item::Text { at, height_mm, angle: 0.0, anchor, text }
}

fn scene(paper: [f64; 2], view: Vec<Item>, notes: Vec<Item>) -> Scene {
    Scene {
        size_mm: Vec2::new(paper[0], paper[1]),
        groups: vec![Group { object: 1, kind: ObjectKind::View, items: view }, Group { object: 2, kind: ObjectKind::Annotation, items: notes }],
    }
}

/// A row's letter, counted from the top (A, B, …; past Z by number).
fn row_name(from_top: usize) -> String {
    u8::try_from(from_top).ok().filter(|k| *k < 26).map_or_else(|| format!("R{}", from_top + 1), |k| char::from(b'A' + k).to_string())
}

/// One part's tiles: the grid's columns and rows, the tile size and where the grid starts.
struct Tiles {
    lo: Vec2,
    size: Vec2,
    cols: usize,
    rows: usize,
    /// The tiles printed, as (column, row from the bottom).
    used: Vec<(usize, usize)>,
}

impl Tiles {
    fn name(&self, short: &str, c: usize, r: usize) -> String {
        format!("{short}-{}{}", row_name(self.rows - 1 - r), c + 1)
    }
    fn corner(&self, c: usize, r: usize) -> Vec2 {
        self.lo + Vec2::new(self.size.x * c as f64, self.size.y * r as f64)
    }
}

fn tiles(part: &Part, d: &Drawn, paper: [f64; 2]) -> Option<Tiles> {
    let first = *part.piece.outline.first()?;
    let (lo, hi) = part
        .piece
        .outline
        .iter()
        .fold((first, first), |(lo, hi), q| (Vec2::new(lo.x.min(q.x), lo.y.min(q.y)), Vec2::new(hi.x.max(q.x), hi.y.max(q.y))));
    if ![lo.x, lo.y, hi.x, hi.y].iter().all(|v| v.is_finite()) {
        return None;
    }
    let lo = Vec2::new(lo.x.min(0.0), lo.y.min(0.0));
    let size = Vec2::new(paper[0] - 2.0 * MARGIN, paper[1] - 2.0 * MARGIN);
    let span = |a: f64, b: f64, s: f64| ((b - a) / s).ceil().clamp(1.0, MOST_PAGES as f64) as usize;
    let (cols, rows) = (span(lo.x, hi.x, size.x), span(lo.y, hi.y, size.y));
    if cols.saturating_mul(rows) > MOST_PAGES {
        return None;
    }
    let mut t = Tiles { lo, size, cols, rows, used: Vec::new() };
    for r in 0..rows {
        for c in 0..cols {
            let (a, b) = (t.corner(c, r), t.corner(c, r) + size);
            let any = d.cut.iter().any(|(p, q)| clip(*p, *q, a, b).is_some()) || d.marks.iter().any(|(p, q, _)| clip(*p, *q, a, b).is_some());
            if any {
                t.used.push((c, r));
            }
        }
    }
    Some(t)
}

/// A page of one tile at full size.
fn tile_page(part: &Part, d: &Drawn, t: &Tiles, c: usize, r: usize, paper: [f64; 2]) -> Scene {
    let (a, b) = (t.corner(c, r), t.corner(c, r) + t.size);
    let on_page = |q: Vec2| q - a + Vec2::new(MARGIN, MARGIN);
    let (mut view, mut notes) = (Vec::new(), Vec::new());
    for (p, q) in &d.cut {
        if let Some((p, q)) = clip(*p, *q, a, b) {
            view.push(stroke(on_page(p), on_page(q), CUT));
        }
    }
    for (p, q, w) in &d.marks {
        if let Some((p, q)) = clip(*p, *q, a, b) {
            view.push(stroke(on_page(p), on_page(q), *w));
        }
    }
    for (at, label) in &d.labels {
        if at.x >= a.x && at.x <= b.x - 20.0 && at.y >= a.y && at.y <= b.y - 3.0 {
            notes.push(text(on_page(*at), 2.2, Anchor::Start, label.clone()));
        }
    }
    // A cross every 100 mm, with where it is on the piece.
    let first = |v: f64| (v / 100.0).ceil() as i64;
    let last = |v: f64| (v / 100.0).floor() as i64;
    for gx in first(a.x)..=last(b.x) {
        for gy in first(a.y)..=last(b.y) {
            let g = on_page(Vec2::new(gx as f64 * 100.0, gy as f64 * 100.0));
            view.push(stroke(g - Vec2::new(3.0, 0.0), g + Vec2::new(3.0, 0.0), FINE));
            view.push(stroke(g - Vec2::new(0.0, 3.0), g + Vec2::new(0.0, 3.0), FINE));
            notes.push(text(g + Vec2::new(1.0, 1.0), 1.8, Anchor::Start, format!("{}, {}", gx * 100, gy * 100)));
        }
    }
    // The trim border: the tile's edge.
    let (p0, p1) = (Vec2::new(MARGIN, MARGIN), Vec2::new(paper[0] - MARGIN, paper[1] - MARGIN));
    for (p, q) in [(p0, Vec2::new(p1.x, p0.y)), (Vec2::new(p1.x, p0.y), p1), (p1, Vec2::new(p0.x, p1.y)), (Vec2::new(p0.x, p1.y), p0)] {
        view.push(stroke(p, q, TRIM));
    }
    let name = t.name(&part.short, c, r);
    let top = paper[1] - 7.0;
    notes.push(text(
        Vec2::new(MARGIN, top),
        3.4,
        Anchor::Start,
        format!(
            "{name}   {}, row {} of {} from the top, column {} of {}   UP ^",
            part.name,
            row_name(t.rows - 1 - r),
            row_name(t.rows - 1),
            c + 1,
            t.cols
        ),
    ));
    notes.push(text(Vec2::new(paper[0] - MARGIN, top), 2.4, Anchor::End, "Seen from the room: tape to the face that faces the cabin"));
    // The scale bar and the neighbours.
    let y = 6.0;
    view.push(stroke(Vec2::new(MARGIN, y), Vec2::new(MARGIN + 100.0, y), 0.3));
    for x in [0.0, 50.0, 100.0] {
        view.push(stroke(Vec2::new(MARGIN + x, y - 1.5), Vec2::new(MARGIN + x, y + 1.5), 0.3));
    }
    notes.push(text(Vec2::new(MARGIN + 104.0, y - 0.8), 2.3, Anchor::Start, "100 mm: check it before cutting (print at Actual size / 100%)"));
    let near: Vec<String> = [(-1i64, 0i64, "left"), (1, 0, "right"), (0, 1, "above"), (0, -1, "below")]
        .iter()
        .filter_map(|(dc, dr, word)| {
            let (nc, nr) = (usize::try_from(c as i64 + dc).ok()?, usize::try_from(r as i64 + dr).ok()?);
            t.used.contains(&(nc, nr)).then(|| format!("{word} {}", t.name(&part.short, nc, nr)))
        })
        .collect();
    if !near.is_empty() {
        notes.push(text(Vec2::new(paper[0] - MARGIN, y - 0.8), 2.3, Anchor::End, near.join("   ")));
    }
    scene(paper, view, notes)
}

/// A map of one part's pages: its outline small, each printed page's box and name.
fn map_page(part: &Part, d: &Drawn, t: &Tiles, paper: [f64; 2]) -> Scene {
    let (mut view, mut notes) = (Vec::new(), Vec::new());
    let top = paper[1] - MARGIN;
    let [w, h] = part.piece.blank;
    notes.push(text(
        Vec2::new(MARGIN, top - 4.0),
        6.0,
        Anchor::Start,
        format!("{}  {w:.0} x {h:.0}: full size on {} pages", part.name, t.used.len()),
    ));
    let mut lines = vec![
        "Print at Actual size / 100% (no fit to page) and check the 100 mm bar on a page. Pages are printed only where there is something to cut or drill."
            .to_owned(),
        "Trim each page along its border on the right and top, lay it over its neighbours so the borders meet, and tape. The crosses every 100 mm give where they are on the piece, measured from its bottom-left corner, so a page can also be placed by tape measure."
            .to_owned(),
        "Thick line: cut. Circles: drill at the size given, on the centre cross.".to_owned(),
    ];
    lines.extend(part.note.clone());
    let mut y = top - 11.0;
    for line in lines.iter().flat_map(|l| wrap(l, 170)) {
        notes.push(text(Vec2::new(MARGIN, y), 2.6, Anchor::Start, line));
        y -= 3.4;
    }
    let grid_hi = t.corner(t.cols, t.rows);
    let room_lo = Vec2::new(MARGIN + 6.0, MARGIN + 6.0);
    let room_hi = Vec2::new(paper[0] - MARGIN - 6.0, y - 4.0);
    let size = grid_hi - t.lo;
    let room = room_hi - room_lo;
    let scale = (room.x / size.x.max(1e-9)).min(room.y / size.y.max(1e-9)).max(1e-9);
    let shift = room_lo + (room - size * scale) * 0.5 - t.lo * scale;
    let at = |q: Vec2| q * scale + shift;
    for (p, q) in &d.cut {
        view.push(stroke(at(*p), at(*q), 0.3));
    }
    for &(c, r) in &t.used {
        let (a, b) = (at(t.corner(c, r)), at(t.corner(c, r) + t.size));
        for (p, q) in [(a, Vec2::new(b.x, a.y)), (Vec2::new(b.x, a.y), b), (b, Vec2::new(a.x, b.y)), (Vec2::new(a.x, b.y), a)] {
            view.push(stroke(p, q, 0.1));
        }
        let mid = (a + b) * 0.5;
        notes.push(text(mid - Vec2::new(0.0, 1.0), ((b.y - a.y) * 0.25).clamp(1.2, 4.0), Anchor::Middle, t.name(&part.short, c, r)));
    }
    scene(paper, view, notes)
}

/// The print's first page: what it is and each part's pages.
fn cover_page(parts: &[(&Part, usize)], paper: [f64; 2]) -> Scene {
    let top = paper[1] - MARGIN;
    let mut notes = vec![text(Vec2::new(MARGIN, top - 6.0), 7.0, Anchor::Start, "Full-size print of the panel's pieces")];
    let mut lines = vec![
        "Each piece's outline, cut-outs, holes and fixing points at 1:1, over as many pages as it takes. Print at Actual size / 100% (no fit to page) and check the 100 mm bar on a page before cutting.".to_owned(),
        "Drawn seen from the room: tape the pages to the face of the board that will face the cabin. The 3 mm gap round each piece is already taken off.".to_owned(),
        String::new(),
    ];
    for (part, pages) in parts {
        let [w, h] = part.piece.blank;
        lines.push(format!("{}  ({})   {w:.0} x {h:.0}   {pages} page{}", part.name, part.short, if *pages == 1 { "" } else { "s" }));
    }
    let mut y = top - 16.0;
    for line in lines.iter().flat_map(|l| if l.is_empty() { vec![String::new()] } else { wrap(l, 140) }) {
        if y < MARGIN {
            break;
        }
        if !line.is_empty() {
            notes.push(text(Vec2::new(MARGIN, y), 3.2, Anchor::Start, line));
        }
        y -= 4.4;
    }
    scene(paper, Vec::new(), notes)
}

/// The print's pages: a cover, then for each part a map of its pages (when it has more than one)
/// and its pages, top row first. `Err` when it would be more pages than [`MOST_PAGES`].
pub(super) fn pages(parts: &[Part], paper: [f64; 2], sizes: &[f64], fixing: f64) -> Result<Vec<Scene>, String> {
    let mut out = Vec::new();
    let mut counts = Vec::new();
    let mut body = Vec::new();
    for part in parts {
        let d = drawn(part, sizes, fixing);
        let Some(mut t) = tiles(part, &d, paper) else {
            return Err(format!("{} would need more than {MOST_PAGES} pages", part.name));
        };
        t.used.sort_by_key(|&(c, r)| (std::cmp::Reverse(r), c));
        if t.used.len() > 1 {
            body.push(map_page(part, &d, &t, paper));
        }
        for &(c, r) in &t.used {
            body.push(tile_page(part, &d, &t, c, r, paper));
        }
        if body.len() > MOST_PAGES {
            return Err(format!("the print would be more than {MOST_PAGES} pages"));
        }
        counts.push((part, t.used.len()));
    }
    out.push(cover_page(&counts, paper));
    out.extend(body);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line is cut to the box, a line outside it is not drawn, one along its edge is kept.
    #[test]
    fn lines_are_clipped_to_the_tile() {
        let (lo, hi) = (Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0));
        let (a, b) = clip(Vec2::new(-5.0, 5.0), Vec2::new(15.0, 5.0), lo, hi).unwrap();
        assert!(a.dist(Vec2::new(0.0, 5.0)) < 1e-9 && b.dist(Vec2::new(10.0, 5.0)) < 1e-9);
        assert!(clip(Vec2::new(-5.0, 20.0), Vec2::new(15.0, 20.0), lo, hi).is_none());
        assert!(clip(Vec2::new(-5.0, -5.0), Vec2::new(-1.0, 15.0), lo, hi).is_none());
        assert!(clip(Vec2::new(0.0, 2.0), Vec2::new(0.0, 8.0), lo, hi).is_some());
    }

    /// A 600 × 300 piece with a hole in the middle on Letter: 3 × 2 tiles; the middle ones hold
    /// the edges, so every tile is printed; a cover and a map come first. A piece too big for the
    /// page budget is refused.
    #[test]
    fn a_piece_is_tiled_with_a_map_and_a_cover() {
        let piece = panel::Piece {
            number: 1,
            origin: Vec2::new(0.0, 0.0),
            outline: vec![Vec2::new(0.0, 0.0), Vec2::new(600.0, 0.0), Vec2::new(600.0, 300.0), Vec2::new(0.0, 300.0)],
            inside: Vec::new(),
            points: Vec::new(),
            blank: [600.0, 300.0],
        };
        let part = Part { name: "Piece 1".into(), short: "P1".into(), piece: &piece, note: None };
        let paper = paper_of(None).unwrap().unwrap();
        let pages = pages(&[part], paper, &[], 8.0).unwrap();
        assert_eq!(pages.len(), 1 + 1 + 6);
        assert!(pages.iter().all(|p| p.validate().is_ok()));
        let huge = panel::Piece { outline: vec![Vec2::new(0.0, 0.0), Vec2::new(1e6, 0.0), Vec2::new(1e6, 1e6)], blank: [1e6, 1e6], ..piece.clone() };
        let part = Part { name: "Piece 2".into(), short: "P2".into(), piece: &huge, note: None };
        assert!(super::pages(&[part], paper, &[], 8.0).is_err());
        assert!(
            paper_of(Some("a4")).is_some_and(|p| p.is_ok()) && paper_of(Some("none")).is_none() && paper_of(Some("A0")).is_some_and(|p| p.is_err())
        );
    }
}
