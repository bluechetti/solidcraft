//! Wall Thickness (`scan.wall_thickness`): how thick a scanned part's walls are, read as a probe
//! reads them. Changes nothing.

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_scan::metrology::{self, Miss, Reading, WallSettings};

use super::{CommandSpec, length, mesh};
use crate::params::{bad, bool_, num};
use crate::schema::scan_thickness::{ScanWallProbe, ScanWallReading, ScanWallThicknessParams, ScanWallThicknessResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanWallThicknessParams> = TypedContract::new();

/// The places read when neither probes nor a number of samples are given.
const DEFAULT_SAMPLES: usize = 2000;
/// The most places one call reads.
const MOST_SAMPLES: usize = 200_000;

pub static COMMANDS: &[CommandSpec] = &[CommandSpec::new("scan.wall_thickness", "Wall Thickness", crate::checked!("scan.wall_thickness", wall_thickness))
    .icon("measure")
    .contract(&CONTRACT)
    .output(output_schema_for::<ScanWallThicknessResult>)
    .params("body?: mesh body (default: the selected or only one); probes?: [[x,y,z], …] points on the scan to read the wall at (the nearest point of the scan is used); samples?: 1–200000 places read over the whole surface, spread evenly by area (default 2000 when there are no probes, else none); flip?: bool (default false: the scan is wound inside out, its rays go the other way: say so when most rays report looking_back); up_to?: length (a ray that goes further is no wall: a solid block, the far side of the part; default no limit); below?: length (list the places where the wall is thinner, and the share of the surface they make): from each place a ray goes straight into the material, square to the face, and where it leaves is the other face; read only where the face it finds looks away from the ray, so a gap is never taken for a wall; changes nothing → body, measured, no_wall, looking_back, thinnest?, thickest?, mean, p05, median, p95 (mm), share_below?, thin? [{at, across, normal, thickness}] (thinnest first, at most 20, none within three times `below` of a thinner one), probes? [{point, reading?, miss?: no_wall|looking_back|no_face}], note?")];

fn xyz(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

fn reading(r: &Reading) -> ScanWallReading {
    ScanWallReading { at: xyz(r.at), across: xyz(r.across), normal: xyz(r.normal), thickness: r.thickness }
}

/// A length given as `key`, if it is.
fn maybe_length(s: &Session, p: &Value, key: &str, cmd: &str) -> Result<Option<f64>> {
    if p.get(key).is_none_or(Value::is_null) {
        return Ok(None);
    }
    length(s, p, key, 0.0, cmd).map(Some)
}

fn wall_thickness(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.wall_thickness";
    let probes: Vec<Vec3> = match p.get("probes").filter(|v| !v.is_null()) {
        Some(v) => v
            .as_array()
            .ok_or_else(|| bad(cmd, "`probes` must list points [[x, y, z], …]"))?
            .iter()
            .map(|q| crate::params::vec3(q).filter(|q| q.is_finite()).ok_or_else(|| bad(cmd, "a probe must be a point [x, y, z] of numbers")))
            .collect::<Result<_>>()?,
        None => Vec::new(),
    };
    let samples = match p.get("samples").filter(|v| !v.is_null()) {
        Some(_) => {
            let n = num(p, "samples").filter(|n| n.fract() == 0.0 && (1.0..=MOST_SAMPLES as f64).contains(n));
            n.ok_or_else(|| bad(cmd, format!("`samples` must be a whole number from 1 to {MOST_SAMPLES}")))? as usize
        }
        None if probes.is_empty() => DEFAULT_SAMPLES,
        None => 0,
    };
    let flip = bool_(p, "flip").unwrap_or(false);
    let (up_to, below) = (maybe_length(s, p, "up_to", cmd)?, maybe_length(s, p, "below", cmd)?);
    let scan = mesh(s, p, cmd)?;
    let found =
        metrology::wall(&scan.scan(), &WallSettings { probes: probes.clone(), samples, flip, up_to, below }).map_err(|e| bad(cmd, e.to_string()))?;

    let probes_out = probes
        .iter()
        .zip(&found.probes)
        .map(|(at, got)| match got {
            Ok(r) => ScanWallProbe { point: xyz(*at), reading: Some(reading(r)), miss: None },
            Err(m) => ScanWallProbe {
                point: xyz(*at),
                reading: None,
                miss: Some(
                    match m {
                        Miss::NoWall => "no_wall",
                        Miss::LookingBack => "looking_back",
                        Miss::NoFace => "no_face",
                    }
                    .to_owned(),
                ),
            },
        })
        .collect();
    let rays = found.measured + found.no_wall + found.looking_back;
    let note = if samples > 0 && found.looking_back * 2 > rays {
        Some("most rays met a face looking back at them: the scan is probably wound inside out; run it again with flip: true".to_owned())
    } else if samples > 0 && found.measured == 0 {
        Some(match up_to {
            Some(_) => {
                "no ray found a wall within `up_to`: it is too short for this scan, or the scan is one open surface with no other side".to_owned()
            }
            None => "no ray found another face: the scan is one open surface, with no other side to read a wall from".to_owned(),
        })
    } else if samples > 0 && found.measured < rays / 2 {
        Some(format!(
            "{} of {rays} places read no wall (an open scan has none at its edges; a short `up_to` leaves out the thick parts)",
            rays - found.measured
        ))
    } else {
        None
    };
    let reply = ScanWallThicknessResult {
        body: scan.body.clone(),
        measured: found.measured,
        no_wall: found.no_wall,
        looking_back: found.looking_back,
        thinnest: found.thinnest.as_ref().map(reading),
        thickest: found.thickest.as_ref().map(reading),
        mean: found.mean,
        p05: found.p05,
        median: found.median,
        p95: found.p95,
        share_below: found.share_below,
        thin: found.thin.iter().map(reading).collect(),
        probes: probes_out,
        note,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
#[path = "scan_thickness_tests.rs"]
mod tests;
