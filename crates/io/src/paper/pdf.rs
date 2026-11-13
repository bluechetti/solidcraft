//! Paper scenes as one PDF, a page each (PDF 1.4): one point a 72nd of an inch, the scene's
//! millimetres scaled to it; y up as the scene has it, so nothing is flipped.
//!
//! - **Curves:** lines `l`; circular arcs as cubics, in pieces of at most a quarter turn (the
//!   usual 4/3·tan(θ/4) handles, within 3e-4 of the radius); cubics `c`; parabolas raised to
//!   cubics. Ellipses, hyperbolas and Béziers above degree 3 are refused, as the SVG writer does.
//! - **Paint:** strokes with butt caps, miter joins and a miter limit of 4; fills black,
//!   even-odd (`f*`).
//! - **Text:** Helvetica, one of the fonts every PDF reader has, WinAnsi-encoded: ASCII and the
//!   Latin-1 letters as they are, anything else written `?`. A centred or right-anchored text is
//!   placed by an estimate of its width (its letters' average widths), not the font's metrics.
//! - **Structure:** the catalogue, the page tree, the font, then each page and its content
//!   stream (not compressed), the cross-reference table and the trailer.

use std::fmt::Write as _;

use solidcore_record::{Seg2, Vec2};
use solidcraft_geom::paper::{Anchor, Item, Prepared, arc_pieces};
use solidcraft_geom::prelude::*;

use crate::{IoError, MAX_DRAWING_BYTES, Result};

/// Points a millimetre.
const PT: f64 = 72.0 / 25.4;

/// A number in points, to the hundredth (a thirtieth of a millimetre... finer than a printer).
fn num(out: &mut String, v: f64) {
    let v = (v * 100.0).round() / 100.0;
    let v = if v == 0.0 { 0.0 } else { v };
    let _ = write!(out, "{v}");
}

fn point(out: &mut String, p: Vec2) {
    num(out, p.x * PT);
    out.push(' ');
    num(out, p.y * PT);
}

/// A piece of a path, from where the path is.
fn piece(out: &mut String, seg: &Seg2) -> Result<()> {
    match *seg {
        Seg2::Line { b, .. } => {
            point(out, b);
            out.push_str(" l\n");
        }
        Seg2::Arc { center, radius, start, sweep } => {
            let mut from = start;
            let n = ((sweep.abs() / std::f64::consts::FRAC_PI_2) - 1e-9).ceil().clamp(1.0, 4.0);
            let step = sweep / n;
            for end in arc_pieces(center, radius, start, sweep) {
                let to = from + step;
                let k = 4.0 / 3.0 * (step / 4.0).tan() * radius;
                let (s0, c0) = from.sin_cos();
                let (s1, c1) = to.sin_cos();
                let p0 = center + Vec2::new(c0, s0) * radius;
                let h0 = p0 + Vec2::new(-s0, c0) * k;
                let h1 = end - Vec2::new(-s1, c1) * k;
                point(out, h0);
                out.push(' ');
                point(out, h1);
                out.push(' ');
                point(out, end);
                out.push_str(" c\n");
                from = to;
            }
        }
        Seg2::Cubic { p1, p2, p3, .. } => {
            point(out, p1);
            out.push(' ');
            point(out, p2);
            out.push(' ');
            point(out, p3);
            out.push_str(" c\n");
        }
        Seg2::Conic { a, apex, b, w } if (w - 1.0).abs() <= 1e-12 => {
            let c1 = a + (apex - a) * (2.0 / 3.0);
            let c2 = b + (apex - b) * (2.0 / 3.0);
            point(out, c1);
            out.push(' ');
            point(out, c2);
            out.push(' ');
            point(out, b);
            out.push_str(" c\n");
        }
        Seg2::Conic { .. } | Seg2::Bezier { .. } => {
            return Err(IoError::Invalid("not supported yet: PDF of ellipses, hyperbolas and Béziers above degree 3".into()));
        }
    }
    Ok(())
}

fn path(out: &mut String, segs: &[Seg2]) -> Result<()> {
    let Some(first) = segs.first() else { return Ok(()) };
    point(out, first.start());
    out.push_str(" m\n");
    for seg in segs {
        piece(out, seg)?;
    }
    Ok(())
}

/// A text's width, roughly, in its own height: Helvetica's letters run about this wide on average.
fn width_estimate(text: &str) -> f64 {
    text.chars()
        .map(|c| match c {
            ' ' | 'i' | 'j' | 'l' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 0.28,
            'f' | 't' | 'r' | 'I' | '(' | ')' | '-' | '/' => 0.33,
            'm' | 'w' | 'M' | 'W' => 0.83,
            c if c.is_ascii_uppercase() => 0.67,
            c if c.is_ascii_digit() => 0.556,
            _ => 0.52,
        })
        .sum()
}

/// A text as a PDF string in WinAnsi: ASCII and Latin-1 as they are, the rest `?`.
fn pdf_string(out: &mut String, text: &str) {
    out.push('(');
    for c in text.chars() {
        match c {
            '(' | ')' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            ' '..='~' => out.push(c),
            '\u{a0}'..='\u{ff}' => {
                let _ = write!(out, "\\{:03o}", u32::from(c));
            }
            _ => out.push('?'),
        }
    }
    out.push(')');
}

/// The content stream of one page.
fn content(scene: &Prepared) -> Result<String> {
    let mut out = String::from("0 J 0 j 4 M 0 g 0 G\n");
    for group in scene.groups() {
        for item in &group.items {
            match item {
                Item::Stroke { path: segs, closed, width_mm } => {
                    num(&mut out, width_mm * PT);
                    out.push_str(" w\n");
                    path(&mut out, segs)?;
                    out.push_str(if *closed { "h S\n" } else { "S\n" });
                }
                Item::Fill { region } => {
                    for l in region.loops() {
                        path(&mut out, &l.segs)?;
                        out.push_str("h\n");
                    }
                    out.push_str("f*\n");
                }
                Item::Text { at, height_mm, angle, anchor, text } => {
                    let size = height_mm * PT;
                    let shift = match anchor {
                        Anchor::Start => 0.0,
                        Anchor::Middle => width_estimate(text) * height_mm / 2.0,
                        Anchor::End => width_estimate(text) * height_mm,
                    };
                    let (s, c) = angle.sin_cos();
                    let start = *at - Vec2::new(c, s) * shift;
                    out.push_str("BT /F1 ");
                    num(&mut out, size);
                    out.push_str(" Tf ");
                    for v in [c, s, -s, c] {
                        let _ = write!(out, "{:.6} ", if v.abs() < 1e-12 { 0.0 } else { v });
                    }
                    point(&mut out, start);
                    out.push_str(" Tm ");
                    pdf_string(&mut out, text);
                    out.push_str(" Tj ET\n");
                }
            }
            if out.len() > MAX_DRAWING_BYTES {
                return Err(IoError::Invalid(format!("the PDF would be over {} MiB", MAX_DRAWING_BYTES >> 20)));
            }
        }
    }
    Ok(out)
}

/// The scenes as one PDF, a page each in order.
pub fn paper_pdf(pages: &[Prepared]) -> Result<Vec<u8>> {
    if pages.is_empty() {
        return Err(IoError::Invalid("a PDF needs at least one page".into()));
    }
    let mut objects: Vec<String> = Vec::new();
    // 1 catalogue, 2 page tree, 3 font; then each page and its content.
    let kids: Vec<String> = (0..pages.len()).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".into());
    objects.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages.len()));
    objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".into());
    let mut total = 0usize;
    for (i, page) in pages.iter().enumerate() {
        let size = page.size_mm();
        let mut media = String::new();
        num(&mut media, size.x * PT);
        media.push(' ');
        num(&mut media, size.y * PT);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {media}] /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + 2 * i
        ));
        let body = content(page)?;
        total += body.len();
        if total > MAX_DRAWING_BYTES {
            return Err(IoError::Invalid(format!("the PDF would be over {} MiB", MAX_DRAWING_BYTES >> 20)));
        }
        objects.push(format!("<< /Length {} >>\nstream\n{body}endstream", body.len()));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (k, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        let _ = write!(out, "{} 0 obj\n{obj}\nendobj\n", k + 1);
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for o in offsets {
        let _ = writeln!(out, "{o:010} 00000 n ");
    }
    let _ = write!(out, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", objects.len() + 1);
    Ok(out.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcraft_geom::paper::{Group, ObjectKind, Scene};

    fn p(x: f64, y: f64) -> Vec2 {
        Vec2::new(x, y)
    }

    fn page(items: Vec<Item>) -> Scene {
        Scene { size_mm: p(279.4, 215.9), groups: vec![Group { object: 1, kind: ObjectKind::View, items }] }
    }

    /// Two pages: a square, a circle and a centred label on one, a filled square on the other. The
    /// file has both pages at Letter size, the font, every object where the cross-reference says,
    /// and the text and paths in points.
    #[test]
    fn scenes_become_a_pdf_a_page_each() {
        let square = vec![
            Seg2::Line { a: p(10.0, 10.0), b: p(110.0, 10.0) },
            Seg2::Line { a: p(110.0, 10.0), b: p(110.0, 110.0) },
            Seg2::Line { a: p(110.0, 110.0), b: p(10.0, 110.0) },
            Seg2::Line { a: p(10.0, 110.0), b: p(10.0, 10.0) },
        ];
        let one = page(vec![
            Item::Stroke { path: square.clone(), closed: true, width_mm: 0.35 },
            Item::Stroke {
                path: vec![Seg2::Arc { center: p(60.0, 60.0), radius: 4.0, start: 0.0, sweep: std::f64::consts::TAU }],
                closed: true,
                width_mm: 0.25,
            },
            Item::Text {
                at: p(60.0, 120.0),
                height_mm: 5.0,
                angle: 0.0,
                anchor: Anchor::Middle,
                text: "PIECE 1 (2440 x 407) \u{d7} \u{2014}".into(),
            },
        ]);
        let two = page(vec![Item::Fill { region: solidcore_record::Region2 { outer: solidcore_record::Loop2 { segs: square }, holes: vec![] } }]);
        let pdf = paper_pdf(&[one.prepare().unwrap(), two.prepare().unwrap()]).unwrap();
        let text = String::from_utf8(pdf.clone()).unwrap();
        assert!(text.starts_with("%PDF-1.4\n") && text.ends_with("%%EOF\n"));
        assert_eq!(text.matches("/Type /Page ").count(), 2);
        assert!(text.contains("/MediaBox [0 0 792 612]"), "Letter landscape in points");
        assert!(text.contains("/BaseFont /Helvetica"));
        // 10 mm is 28.35 pt; a quarter circle is one cubic, so four for the hole.
        assert!(text.contains("28.35 28.35 m"), "{text}");
        assert_eq!(text.matches(" c\n").count(), 4);
        // The text escaped: parentheses, the times sign as Latin-1, the dash as `?`.
        assert!(text.contains("(PIECE 1 \\(2440 x 407\\) \\327 ?) Tj"), "{text}");
        assert!(text.contains("f*"));
        // Each object where the cross-reference says it is.
        let xref = text.rfind("xref\n").unwrap();
        for (k, line) in text[xref..].lines().skip(3).take(7).enumerate() {
            let at: usize = line[..10].parse().unwrap();
            assert!(text[at..].starts_with(&format!("{} 0 obj", k + 1)), "object {}", k + 1);
        }
        assert!(paper_pdf(&[]).is_err());
    }
}
