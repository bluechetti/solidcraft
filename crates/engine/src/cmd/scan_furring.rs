//! Furring strips (`scan.furring`, `scan.furring_proposals`): strips for a scanned wall or ceiling,
//! bolted through the frame's own holes, each strip's back following the scanned metal and its
//! front on one plane per band, so flat panels screw straight on. Each strip is cut into segments
//! a 3D printer takes; `scan.furring` makes them mesh bodies (one Insert Mesh) and can write their
//! STL files and a CSV.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;
use solidcraft_scan::furring::{self, Options, Plan};

use super::{CommandSpec, mesh};
use crate::params::bad;
use crate::schema::scan_furring::{
    FurringBand, FurringBatten, FurringHole, FurringProud, FurringSegment, FurringStrip, FurringWidth, ScanFurringParams, ScanFurringResult,
};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanFurringParams> = TypedContract::new();

/// Heights the clear width is given at by default: this far apart (mm).
const WIDTH_STEP: f64 = 50.0;
/// The most clear widths given.
const MOST_WIDTHS: usize = 500;

macro_rules! params {
    () => {
        "body?: the wall's scan (one wall or the ceiling; default the selected or only one); up?: [x,y,z] the way the strips run (default [0,0,1]); toward?: [x,y,z] a point in the room; bands?: [[from, to], …] heights along up (default: found where the wall's front steps 30 mm or more); lean?: bool (default true: each band's plane leans with the wall up to 6°); tight?: 0.5–1 (default 0.98: the share of each row of the wall left behind the plane); standoff?: mm (default 0: flush, the panel's back on the frame itself, strips only where the metal lies behind the plane, each bolted through existing holes there); width?, thickness?, deepest?: mm (default 40, 8, 25); gap?: mm (default 0.5, the print gap off the metal); panel?: mm (default 6); batten?, batten_width?, batten_spacing?: mm (default 19, 38, 400: wooden battens on the brackets' plane, square to the brackets' main way, each across two or more brackets; batten 0: none, strips fill behind the panel instead); bolt?, head?, head_depth?: mm (default 6.6, 13, 5: a counterbored clearance hole at each of the frame's holes); segment?: mm (default 330, the longest printed piece); spacing?: mm (default 250: strips no closer); slant?: degrees (default 3: how far a strip may lean to reach its holes; each strip runs from a hole to a hole through all the bands, its front stepping with their planes); cell?: mm (default 2); skip?: [[[x,y,z],[x,y,z]], …] rectangles on the wall left out (a wheel arch, a door pillar), each by two opposite corners; label?: the segments' names start with this (default S); opposite?: the facing wall's scan and heights?: [h, …] (the clear width between the finished faces)"
    };
}

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.furring_proposals", "Furring Proposals", crate::checked!("scan.furring_proposals", furring_proposals))
        .icon("flat")
        .contract(&CONTRACT)
        .output(output_schema_for::<ScanFurringResult>)
        .params(concat!(params!(), ": the strips worked out, the design unchanged → body, x_axis, up, normal, bands[{from, to, face_point, face_normal, lean, raised, holes_found, proud[{center, most, area, size}]}], strips[{name, start, end, length, thinnest, thickest, holes[{along, center, diameter, thickness, counterbore}], segments[{name, from, to, length, holes, volume, triangles}]}], battens[{name, start, end, length, crosses}], unsupported, segments, volume, widths[{height, width}]")),
    CommandSpec::new("scan.furring", "Furring Strips", crate::checked!("scan.furring", furring_make))
        .at("SCAN", "SKETCH")
        .icon("flat")
        .contract(&CONTRACT)
        .output(output_schema_for::<ScanFurringResult>)
        .params(concat!(params!(), "; make?: bool (default true: each segment a mesh body, all in one Insert Mesh `Furring Strips`; one undo step); export?: a folder: each segment's STL (<name>.stl, in place, its flat front the face to print on) and <label>-strips.csv written there → as scan.furring_proposals, and feature, made, files")),
];

fn parse(p: &Value, cmd: &str) -> Result<ScanFurringParams> {
    serde_json::from_value(p.clone()).map_err(|e| bad(cmd, e.to_string()))
}

fn options(q: &ScanFurringParams) -> Options {
    let d = Options::default();
    Options {
        up: q.up.as_ref().map_or(d.up, |p| p.0),
        toward: q.toward.as_ref().map(|p| p.0),
        bands: q.bands.clone().unwrap_or_default(),
        lean: q.lean.unwrap_or(d.lean),
        tight: q.tight.unwrap_or(d.tight),
        standoff: q.standoff.unwrap_or(d.standoff),
        width: q.width.unwrap_or(d.width),
        thickness: q.thickness.unwrap_or(d.thickness),
        deepest: q.deepest.unwrap_or(d.deepest),
        gap: q.gap.unwrap_or(d.gap),
        panel: q.panel.unwrap_or(d.panel),
        batten: q.batten.unwrap_or(d.batten),
        batten_width: q.batten_width.unwrap_or(d.batten_width),
        batten_spacing: q.batten_spacing.unwrap_or(d.batten_spacing),
        bolt: q.bolt.unwrap_or(d.bolt),
        head: q.head.unwrap_or(d.head),
        head_depth: q.head_depth.unwrap_or(d.head_depth),
        segment: q.segment.unwrap_or(d.segment),
        spacing: q.spacing.unwrap_or(d.spacing),
        slant: q.slant.unwrap_or(d.slant),
        cell: q.cell.unwrap_or(d.cell),
        skip: q.skip.as_ref().map(|v| v.iter().map(|[a, b]| [a.0, b.0]).collect()).unwrap_or_default(),
    }
}

fn arr(v: Vec3) -> [f64; 3] {
    [v.x, v.y, v.z]
}

/// The segments' names: `<label><strip>-<segment>`, strips numbered from 1 along the wall.
fn names(plan: &Plan, label: &str) -> Vec<(String, Vec<String>)> {
    plan.strips
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let k = i + 1;
            (format!("{label}{k}"), (1..=s.segments.len()).map(|j| format!("{label}{k}-{j}")).collect())
        })
        .collect()
}

struct Worked {
    body: String,
    plan: Plan,
    label: String,
    result: ScanFurringResult,
}

fn work(s: &Session, p: &Value, cmd: &str) -> Result<Worked> {
    let q = parse(p, cmd)?;
    let label = q.label.clone().filter(|l| !l.trim().is_empty() && l.len() <= 16 && !l.contains(['/', '\\'])).unwrap_or_else(|| "S".into());
    let o = options(&q);
    let on = mesh(s, p, cmd)?;
    let plan = furring::plan(&on.scan(), &o).map_err(|e| bad(cmd, e.to_string()))?;
    let named = names(&plan, &label);
    let f = &plan.wall.frame;
    let on_wall = |q: solidcore_record::Vec2| f.origin + f.x * q.x + f.y * q.y + plan.wall.normal * plan.front(q.y);
    let bands: Vec<FurringBand> = plan
        .bands
        .iter()
        .map(|b| {
            let (point, normal) = plan.face(b, plan.middle(), (b.from + b.to) / 2.0);
            FurringBand {
                from: b.from,
                to: b.to,
                face_point: arr(point),
                face_normal: arr(normal),
                lean: b.lean.atan().to_degrees(),
                raised: b.raised,
                holes_found: b.holes_found,
                proud: b.proud.iter().map(|x| FurringProud { center: arr(x.center), most: x.most, area: x.area, size: x.size }).collect(),
            }
        })
        .collect();
    let strips: Vec<FurringStrip> = plan
        .strips
        .iter()
        .zip(&named)
        .map(|(st, (name, segs))| FurringStrip {
            name: name.clone(),
            start: arr(on_wall(st.start)),
            end: arr(on_wall(st.end)),
            length: st.length(),
            thinnest: st.thinnest,
            thickest: st.thickest,
            holes: st
                .holes
                .iter()
                .map(|h| FurringHole {
                    along: h.along,
                    center: arr(h.center),
                    diameter: h.diameter,
                    thickness: h.thickness,
                    counterbore: h.counterbore,
                })
                .collect(),
            segments: st
                .segments
                .iter()
                .zip(segs)
                .map(|(g, name)| FurringSegment {
                    name: name.clone(),
                    from: g.from,
                    to: g.to,
                    length: g.to - g.from,
                    holes: g.holes.iter().map(|k| k + 1).collect(),
                    volume: g.volume,
                    triangles: g.triangles.len(),
                })
                .collect(),
        })
        .collect();
    let battens: Vec<FurringBatten> = plan
        .battens
        .iter()
        .enumerate()
        .map(|(i, b)| FurringBatten {
            name: format!("{label}B{}", i + 1),
            start: arr(b.start),
            end: arr(b.end),
            length: b.length,
            crosses: b.crosses,
        })
        .collect();
    let segments = plan.strips.iter().map(|s| s.segments.len()).sum();
    let volume = plan.strips.iter().flat_map(|s| &s.segments).map(|g| g.volume).sum();
    let mut widths = Vec::new();
    if let Some(other) = q.opposite.as_deref() {
        let theirs = mesh(s, &json!({ "body": other }), cmd)?;
        let mut oo = o.clone();
        oo.toward = Some(on.scan().bounds().map_or(f.origin, |(lo, hi)| (lo + hi) * 0.5));
        let other_plan = furring::plan(&theirs.scan(), &oo).map_err(|e| bad(cmd, format!("opposite: {e}")))?;
        let heights: Vec<f64> = match &q.heights {
            Some(h) => h.iter().copied().filter(|v| v.is_finite()).take(MOST_WIDTHS).collect(),
            None => {
                let (lo, hi) = plan.bands.iter().fold((f64::MAX, f64::MIN), |(lo, hi), b| (lo.min(b.from), hi.max(b.to)));
                let mut h = (lo / WIDTH_STEP).ceil() * WIDTH_STEP;
                let mut v = Vec::new();
                while h <= hi && v.len() < MOST_WIDTHS {
                    v.push(h);
                    h += WIDTH_STEP;
                }
                v
            }
        };
        for h in heights {
            if let Some(width) = furring::clear_width(&plan, &other_plan, h, None) {
                widths.push(FurringWidth { height: h, width });
            }
        }
    }
    let result = ScanFurringResult {
        body: on.body.clone(),
        x_axis: arr(f.x),
        up: arr(f.y),
        normal: arr(plan.wall.normal),
        bands,
        strips,
        battens,
        unsupported: plan.unsupported,
        segments,
        volume,
        widths,
        feature: None,
        made: Vec::new(),
        files: Vec::new(),
    };
    Ok(Worked { body: on.body, plan, label, result })
}

fn reply<T: serde::Serialize>(r: T) -> Result<Value> {
    serde_json::to_value(r).map_err(|e| EngineError::Other(e.to_string()))
}

fn furring_proposals(s: &mut Session, p: &Value) -> Result<Value> {
    reply(work(s, p, "scan.furring_proposals")?.result)
}

/// A binary STL of one closed mesh.
fn stl(name: &str, positions: &[Vec3], triangles: &[[u32; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(84 + triangles.len() * 50);
    let mut header = format!("SolidCraft furring {name}").into_bytes();
    header.resize(80, b' ');
    out.extend_from_slice(&header);
    out.extend_from_slice(&u32::try_from(triangles.len()).unwrap_or(u32::MAX).to_le_bytes());
    for t in triangles {
        let (Some(a), Some(b), Some(c)) = (positions.get(t[0] as usize), positions.get(t[1] as usize), positions.get(t[2] as usize)) else {
            continue;
        };
        let n = (*b - *a).cross(*c - *a).normalized().unwrap_or_default();
        for v in [n, *a, *b, *c] {
            for x in v.to_f32() {
                out.extend_from_slice(&x.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}

fn furring_make(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.furring";
    let make = p.get("make").and_then(Value::as_bool).unwrap_or(true);
    let export = p.get("export").and_then(Value::as_str).filter(|x| !x.trim().is_empty()).map(str::to_owned);
    let Worked { body, plan, label, mut result } = work(s, p, cmd)?;
    if result.segments == 0 {
        return Err(bad(cmd, "no strips: no run of the frame's holes was found for a strip (try `bands`, a larger `slant` or a lower `spacing`)"));
    }
    let named = names(&plan, &label);
    let segs: Vec<(&String, &furring::Segment)> =
        plan.strips.iter().zip(&named).flat_map(|(st, (_, names))| names.iter().zip(&st.segments)).collect();
    if let Some(dir) = &export {
        let dir = dir.trim_end_matches(['/', '\\']);
        let mut csv = String::from("segment,strip,from,to,length,volume_mm3,holes,thinnest,thickest\n");
        for st in &result.strips {
            for g in &st.segments {
                use std::fmt::Write as _;
                let holes: Vec<String> = g.holes.iter().map(|k| k.to_string()).collect();
                let _ = writeln!(
                    csv,
                    "{},{},{:.1},{:.1},{:.1},{:.0},{},{:.1},{:.1}",
                    g.name,
                    st.name,
                    g.from,
                    g.to,
                    g.length,
                    g.volume,
                    holes.join(" "),
                    st.thinnest,
                    st.thickest
                );
            }
        }
        let mut cut = String::from("batten,length_mm,crosses,start_x,start_y,start_z,end_x,end_y,end_z\n");
        for b in &result.battens {
            use std::fmt::Write as _;
            let _ = writeln!(
                cut,
                "{},{:.0},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1}",
                b.name, b.length, b.crosses, b.start[0], b.start[1], b.start[2], b.end[0], b.end[1], b.end[2]
            );
        }
        let name = format!("{dir}/{label}-battens.csv");
        solidcraft_io::vfs::write(&name, cut.as_bytes()).map_err(|e| EngineError::Other(format!("{name}: {e}")))?;
        result.files.push(name);
        let name = format!("{dir}/{label}-strips.csv");
        solidcraft_io::vfs::write(&name, csv.as_bytes()).map_err(|e| EngineError::Other(format!("{name}: {e}")))?;
        result.files.push(name);
        for (n, g) in &segs {
            let name = format!("{dir}/{n}.stl");
            solidcraft_io::vfs::write(&name, &stl(n, &g.positions, &g.triangles)).map_err(|e| EngineError::Other(format!("{name}: {e}")))?;
            result.files.push(name);
        }
    }
    if make {
        let meshes: Vec<solidcraft_doc::MeshData> = segs
            .iter()
            .map(|(n, g)| solidcraft_doc::MeshData {
                name: (*n).clone(),
                positions: g.positions.iter().flat_map(|q| [q.x, q.y, q.z]).collect::<Vec<f64>>().into(),
                triangles: g.triangles.iter().flatten().copied().collect::<Vec<u32>>().into(),
                corner_normals: Default::default(),
                color: Some([0.95, 0.55, 0.15]),
            })
            .collect();
        let mut meshes = meshes;
        meshes.extend(plan.battens.iter().zip(&result.battens).map(|(b, r)| solidcraft_doc::MeshData {
            name: r.name.clone(),
            positions: b.positions.iter().flat_map(|q| [q.x, q.y, q.z]).collect::<Vec<f64>>().into(),
            triangles: b.triangles.iter().flatten().copied().collect::<Vec<u32>>().into(),
            corner_normals: Default::default(),
            color: Some([0.62, 0.42, 0.24]),
        }));
        result.made = meshes.iter().map(|m| m.name.clone()).collect();
        let kind = solidcraft_doc::FeatureKind::MeshImport { file: format!("{body} (furring)"), meshes };
        let made = super::super::features::add_feature(s, &json!({ "name": "Furring Strips" }), kind)?;
        result.feature = made.get("feature").and_then(Value::as_u64).or_else(|| made.get("id").and_then(Value::as_u64));
    }
    reply(result)
}
