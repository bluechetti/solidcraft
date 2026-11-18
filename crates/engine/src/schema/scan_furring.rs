//! Contracts and result types for the commands in `cmd/scan_furring.rs`: furring strips for a
//! scanned wall or ceiling, to 3D print and bolt through the frame's holes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::types::Point3;

/// The furring for a scanned wall: bands, their planes, strips and printed segments.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanFurringParams {
    /// The wall's scan (one wall, or the ceiling). Default: the selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// The scan of the wall facing it: the clear width between the two finished faces is given at
    /// `heights`.
    #[serde(default)]
    pub opposite: Option<String>,
    /// Heights (along `up`, world mm) the clear width is given at. Default: every 50 mm over the
    /// bands.
    #[serde(default)]
    pub heights: Option<Vec<f64>>,
    /// The way the strips run: [0, 0, 1] (default) for a side wall, across the van for a ceiling.
    #[serde(default)]
    pub up: Option<Point3>,
    /// A point in the room: the wall's front faces it. Default: the side the wall's plane faces.
    #[serde(default)]
    pub toward: Option<Point3>,
    /// The bands' heights [[from, to], …] (along `up`, world mm). Default: found where the wall's
    /// front steps by 30 mm or more.
    #[serde(default)]
    pub bands: Option<Vec<[f64; 2]>>,
    /// Each band's plane leans with the wall, up to 6°. Default: true.
    #[serde(default)]
    pub lean: Option<bool>,
    /// The share of each row of the wall's front left behind the plane (0.5–1). Default: 0.98.
    #[serde(default)]
    pub tight: Option<f64>,
    /// How far the strips' fronts (the panel's back) stand in front of the frame (default 0:
    /// flush, the panel on the frame and strips only where the metal lies behind it).
    #[serde(default)]
    pub standoff: Option<f64>,
    /// The strip's width (default 40 mm), least thickness (8 mm) and the deepest it reaches behind
    /// its front (25 mm; deeper metal is bridged).
    #[serde(default)]
    pub width: Option<f64>,
    #[serde(default)]
    pub thickness: Option<f64>,
    #[serde(default)]
    pub deepest: Option<f64>,
    /// The print gap off the metal (default 0.5 mm).
    #[serde(default)]
    pub gap: Option<f64>,
    /// The panel's thickness (default 6 mm): the finished face is this far in front of the strips.
    #[serde(default)]
    pub panel: Option<f64>,
    /// Battens across the brackets: thickness (default 19 mm; 0: none, the strips fill behind
    /// the panel instead), width (38 mm) and the most between them (400 mm).
    #[serde(default)]
    pub batten: Option<f64>,
    #[serde(default)]
    pub batten_width: Option<f64>,
    #[serde(default)]
    pub batten_spacing: Option<f64>,
    /// The bolt's clearance hole (default 6.6 mm), its head's counterbore (13 mm) and its depth
    /// (5 mm; 0: none).
    #[serde(default)]
    pub bolt: Option<f64>,
    #[serde(default)]
    pub head: Option<f64>,
    #[serde(default)]
    pub head_depth: Option<f64>,
    /// The longest printed segment (default 330 mm).
    #[serde(default)]
    pub segment: Option<f64>,
    /// Strips closer than this (middle to middle) are not both made (default 250 mm).
    #[serde(default)]
    pub spacing: Option<f64>,
    /// The most a strip leans off `up` to reach its holes (default 3°).
    #[serde(default)]
    pub slant: Option<f64>,
    /// The cell the wall's front is read on (default 2 mm).
    #[serde(default)]
    pub cell: Option<f64>,
    /// Rectangles on the wall left out, each two opposite corners [[x, y, z], [x, y, z]]: a wheel
    /// arch, a door pillar.
    #[serde(default)]
    pub skip: Option<Vec<[Point3; 2]>>,
    /// The segments' names start with this (default `S`): `S3-2` is strip 3's second segment.
    #[serde(default)]
    pub label: Option<String>,
    /// With `scan.furring`: a folder the segments' STL files and the strips' CSV are written to.
    #[serde(default)]
    pub export: Option<String>,
    /// With `scan.furring`: make each segment a mesh body (default true).
    #[serde(default)]
    pub make: Option<bool>,
}

/// What `scan.furring` and `scan.furring_proposals` return.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanFurringResult {
    pub body: String,
    /// The wall's frame: along it, up, and its normal to the room.
    pub x_axis: [f64; 3],
    pub up: [f64; 3],
    pub normal: [f64; 3],
    pub bands: Vec<FurringBand>,
    pub strips: Vec<FurringStrip>,
    /// The battens (wood, cut to `length`) and the batten lines left out (fewer than two brackets).
    pub battens: Vec<FurringBatten>,
    pub unsupported: usize,
    /// Segments and their volume all told (mm³).
    pub segments: usize,
    pub volume: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub widths: Vec<FurringWidth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feature: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub made: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
}

/// A band of the wall.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringBand {
    /// Its heights (along `up`, world mm).
    pub from: f64,
    pub to: f64,
    /// Its finished face (strips' fronts plus the panel) at the band's middle, and the face's normal.
    pub face_point: [f64; 3],
    pub face_normal: [f64; 3],
    /// How far the face leans off the wall's frame (degrees, out going up), and how far the plane
    /// was moved out so every strip keeps its thickness (mm).
    pub lean: f64,
    pub raised: f64,
    pub holes_found: usize,
    /// Places where the metal stands in front of the strips' plane (the panel needs a cut-out).
    pub proud: Vec<FurringProud>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringStrip {
    pub name: String,
    /// Its ends on the metal's plane (in space), its length, thinnest and thickest (mm).
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub length: f64,
    pub thinnest: f64,
    pub thickest: f64,
    pub holes: Vec<FurringHole>,
    pub segments: Vec<FurringSegment>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringHole {
    /// How far along the strip (mm), its centre on the metal and its scanned diameter.
    pub along: f64,
    pub center: [f64; 3],
    pub diameter: f64,
    pub thickness: f64,
    pub counterbore: bool,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringSegment {
    pub name: String,
    pub from: f64,
    pub to: f64,
    pub length: f64,
    /// The strip's holes in it (numbered from 1), its volume (mm³) and triangles.
    pub holes: Vec<usize>,
    pub volume: f64,
    pub triangles: usize,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringProud {
    pub center: [f64; 3],
    pub most: f64,
    pub area: f64,
    pub size: [f64; 2],
}

/// The clear width between this wall's finished face and the opposite one's.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringWidth {
    pub height: f64,
    pub width: f64,
}

/// A batten across the brackets.
#[derive(Debug, Serialize, JsonSchema)]
pub struct FurringBatten {
    pub name: String,
    /// Its ends (middle of its back), length (mm) and the brackets it crosses.
    pub start: [f64; 3],
    pub end: [f64; 3],
    pub length: f64,
    pub crosses: usize,
}
