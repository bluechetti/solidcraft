//! Contracts and result types for `scan.wall_thickness` (`cmd/scan_thickness.rs`).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::Point3;

/// Read the wall thickness of a scan: from points on one face (probes) and from places spread
/// over the whole surface, a ray goes straight into the material and the other face is where it
/// leaves. Changes nothing.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ScanWallThicknessParams {
    /// The scan. Default: the selected or only one.
    #[serde(default)]
    pub body: Option<String>,
    /// Points on the scan to read the wall at (the nearest point of the scan is used).
    #[serde(default)]
    pub probes: Option<Vec<Point3>>,
    /// How many places to read over the surface, 1 to 200 000, spread evenly by area. Default:
    /// 2000 when there are no probes, else none.
    #[serde(default)]
    pub samples: Option<u32>,
    /// Go the other way: for a scan wound inside out (normals pointing in).
    #[serde(default)]
    pub flip: Option<bool>,
    /// The longest wall to read, a length: a ray that goes further is no wall (a solid block,
    /// the far side of the part). Default: no limit.
    #[serde(default)]
    pub up_to: Option<Value>,
    /// List the places where the wall is thinner than this, a length, and the share of the surface
    /// they make.
    #[serde(default)]
    pub below: Option<Value>,
}

/// A wall read at one place.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanWallReading {
    /// The point on the face the ray started from.
    pub at: [f64; 3],
    /// Where it left the material, on the other face.
    pub across: [f64; 3],
    /// The first face's direction there, out of the material.
    pub normal: [f64; 3],
    /// The wall's thickness (mm).
    pub thickness: f64,
}

/// What a probe read.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanWallProbe {
    /// The point given.
    pub point: [f64; 3],
    /// The reading, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reading: Option<ScanWallReading>,
    /// Why there is none: `no_wall` (the ray found no face, or none within `up_to`),
    /// `looking_back` (the face it found looks back at the ray, so there is no material between)
    /// or `no_face`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub miss: Option<String>,
}

/// What `scan.wall_thickness` returns.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ScanWallThicknessResult {
    pub body: String,
    /// The places read over the surface that gave a reading, and those that gave none: for want
    /// of a wall (`no_wall`) or a face that looks back (`looking_back`).
    pub measured: usize,
    pub no_wall: usize,
    pub looking_back: usize,
    /// The thinnest and the thickest reading over the surface.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinnest: Option<ScanWallReading>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thickest: Option<ScanWallReading>,
    /// Over the surface's readings: the mean, and the 5th, 50th and 95th percentile (mm).
    pub mean: f64,
    pub p05: f64,
    pub median: f64,
    pub p95: f64,
    /// With `below`: the share of the readings under it, and the thinnest places, thinnest first
    /// (at most 20, none within three times `below` of a thinner one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_below: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thin: Vec<ScanWallReading>,
    /// What each probe read, in the order given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probes: Vec<ScanWallProbe>,
    /// What to know about the readings (most rays met a face looking back, for one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub param_warnings: Vec<String>,
}
