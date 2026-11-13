//! A sheet's paper scene (`geom::paper`) in a file: SVG (A3, #4890) and PDF; DXF and PNG follow.

mod pdf;
mod svg;

pub use pdf::paper_pdf;
pub use svg::{SvgReport, paper_svg};
