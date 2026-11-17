//! Cleaning a scan as a step of the timeline: a Scan Clean feature keeps one clean tool's settings
//! (Remove Floating Pieces, Fill Holes, Reduce Mesh, Repair Mesh, Denoise, Remesh, Remove Markers), never the
//! triangles it makes. The scan itself is kept once, in the Mesh Import that brought it in; each
//! step reads the scan as the steps before it left it and makes it again, so the cleaned scan is
//! worked out when the design opens and again from a step that is edited, suppressed or deleted.
//!
//! A step whose tool finds nothing to do on the scan it now meets (no piece floats any more after
//! an earlier step changed) leaves the scan as it is with a warning, rather than failing.
//!
//! The command that adds a step has already worked out what the step makes, to say what it did:
//! [`clean`] keeps that last result, so the evaluation that follows takes it instead of working it
//! out a second time (a Denoise of ten million triangles takes half a minute).

use std::sync::{Arc, Mutex, Weak};

use serde::{Deserialize, Serialize};
use solidcore_record::Vec3;
use solidcraft_kernel::Body;
use solidcraft_scan::Scan;

use crate::BodyName;

/// The most other scans a Remesh step fuses in.
pub const MAX_WITH: usize = 64;
/// The default share of the largest piece below which a piece floats.
pub const FLOATING_SHARE: f64 = 0.02;
/// The default largest hole Fill Holes closes, in edges round it.
pub const MAX_EDGES: u32 = 300;

/// One clean tool and its settings, as the step keeps them (lengths in mm).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum CleanStep {
    /// Remove Floating Pieces: pieces with fewer triangles than `below` of the largest one's,
    /// but always the pieces under the points `keep` stay and those under `remove` go (clicked).
    RemoveFloating {
        below: f64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        keep: Vec<[f64; 3]>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        remove: Vec<[f64; 3]>,
    },
    /// Fill Holes: every hole of at most `max_edges` edges, shaped to the surface when `fair`; or,
    /// when `at` has points, exactly the rings nearest them (a rim among them, capped), whatever
    /// their size.
    FillHoles {
        max_edges: u32,
        fair: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        at: Vec<[f64; 3]>,
    },
    /// Reduce Mesh: to a share of the triangles, a count, or as far as an error allows; within
    /// `area` only when it has one (the share then of the area's triangles).
    Reduce {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratio: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        triangles: Option<u64>,
        #[serde(default)]
        max_error: f64,
        keep_rim: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<StepArea>,
    },
    /// Repair Mesh: copied triangles, edges of three or more faces, slivers and, when
    /// `crossings`, faces passing through others.
    Repair { crossings: bool },
    /// Denoise: `strength` passes, or as many as the scan's own noise suggests; within `area`
    /// only when it has one.
    Denoise {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strength: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<StepArea>,
    },
    /// Remove Markers: the reference spheres of `radius` at `centres` (found when the step was
    /// made) taken off, and the holes they leave closed, shaped to the surface when `fair`.
    RemoveMarkers { radius: f64, centres: Vec<[f64; 3]>, fair: bool },
    /// Remesh: on a grid of `cell` (the scan's typical edge when none), fusing in the scans
    /// `with`; `sharp` keeps edges and corners sharp. Within `area` only when it has one: its
    /// triangles made again as even triangles of about `cell` on the scan as it was, everything
    /// outside it kept (no scans fused in).
    Remesh {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cell: Option<f64>,
        sharp: bool,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        with: Vec<BodyName>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        area: Option<StepArea>,
    },
}

/// The part of the scan a step works on: triangles of the scan as the step meets it (the steps
/// before it make the same scan every time, so the numbers stand), as runs of consecutive ones,
/// `[first, how many]`, in order, as a kept selection keeps them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, solidcraft_derive::Walk)]
pub struct StepArea {
    /// How many triangles the scan had.
    pub triangles: u32,
    pub runs: Vec<[u32; 2]>,
}

/// The most runs an area keeps.
pub const MAX_AREA_RUNS: usize = 1_000_000;

impl StepArea {
    /// The area of the triangles `chosen` marks (one flag a triangle of the scan).
    pub fn of(chosen: &[bool]) -> StepArea {
        let mut runs: Vec<[u32; 2]> = Vec::new();
        for (t, on) in chosen.iter().enumerate() {
            if !on {
                continue;
            }
            match runs.last_mut() {
                Some([first, count]) if (*first + *count) as usize == t => *count += 1,
                _ => runs.push([t as u32, 1]),
            }
        }
        StepArea { triangles: chosen.len() as u32, runs }
    }

    /// One flag a triangle of the scan it was chosen on.
    pub fn flags(&self) -> Vec<bool> {
        let mut out = vec![false; self.triangles as usize];
        for [first, count] in &self.runs {
            let (a, b) = (*first as usize, (*first as usize).saturating_add(*count as usize).min(out.len()));
            if let Some(span) = out.get_mut(a.min(b)..b) {
                span.fill(true);
            }
        }
        out
    }

    /// How many triangles it holds.
    pub fn count(&self) -> usize {
        self.runs.iter().map(|[_, n]| *n as usize).sum()
    }

    /// Runs in order, apart, within the scan, and not too many.
    fn ok(&self) -> bool {
        self.runs.len() <= MAX_AREA_RUNS
            && self.runs.iter().all(|[a, n]| *n > 0 && a.checked_add(*n).is_some_and(|end| end <= self.triangles))
            && self.runs.windows(2).all(|w| w[0][0] + w[0][1] < w[1][0])
    }
}

impl CleanStep {
    /// The command that makes and edits this step.
    pub fn command(&self) -> &'static str {
        match self {
            CleanStep::RemoveFloating { .. } => "scan.remove_floating",
            CleanStep::FillHoles { .. } => "scan.fill_holes",
            CleanStep::Reduce { .. } => "scan.reduce",
            CleanStep::Repair { .. } => "scan.repair",
            CleanStep::Denoise { .. } => "scan.denoise",
            CleanStep::Remesh { .. } => "scan.remesh",
            CleanStep::RemoveMarkers { .. } => "scan.remove_markers",
        }
    }

    /// The step's name in the timeline, before it is numbered.
    pub fn title(&self) -> &'static str {
        match self {
            CleanStep::RemoveFloating { .. } => "Remove Floating Pieces",
            CleanStep::FillHoles { .. } => "Fill Holes",
            CleanStep::Reduce { .. } => "Reduce Mesh",
            CleanStep::Repair { .. } => "Repair Mesh",
            CleanStep::Denoise { .. } => "Denoise",
            CleanStep::Remesh { .. } => "Remesh",
            CleanStep::RemoveMarkers { .. } => "Remove Markers",
        }
    }

    /// The prefix the step's feature is numbered after (`FillHoles1`).
    pub fn base_name(&self) -> &'static str {
        match self {
            CleanStep::RemoveFloating { .. } => "RemoveFloating",
            CleanStep::FillHoles { .. } => "FillHoles",
            CleanStep::Reduce { .. } => "ReduceMesh",
            CleanStep::Repair { .. } => "RepairMesh",
            CleanStep::Denoise { .. } => "Denoise",
            CleanStep::Remesh { .. } => "Remesh",
            CleanStep::RemoveMarkers { .. } => "RemoveMarkers",
        }
    }

    /// The command's parameters that make this step again (without the scan).
    pub fn params(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            CleanStep::RemoveFloating { below, keep, remove } => {
                let mut p = json!({ "below": below });
                if !keep.is_empty() {
                    p["keep"] = json!(keep);
                }
                if !remove.is_empty() {
                    p["remove"] = json!(remove);
                }
                p
            }
            CleanStep::FillHoles { max_edges, fair, at } => {
                let mut p = json!({ "max_edges": max_edges, "fair": fair });
                if !at.is_empty() {
                    p["at"] = json!(at);
                }
                p
            }
            CleanStep::Reduce { ratio, triangles, max_error, keep_rim, area } => {
                let mut p = json!({ "keep_rim": keep_rim });
                if let Some(a) = area {
                    p["area"] = json!(a);
                }
                if let Some(r) = ratio {
                    p["ratio"] = json!(r);
                }
                if let Some(n) = triangles {
                    p["triangles"] = json!(n);
                }
                if *max_error > 0.0 {
                    p["max_error"] = json!(max_error);
                }
                p
            }
            CleanStep::Repair { crossings } => json!({ "crossings": crossings }),
            CleanStep::Denoise { strength, area } => {
                let mut p = strength.map_or_else(|| json!({}), |k| json!({ "strength": k }));
                if let Some(a) = area {
                    p["area"] = json!(a);
                }
                p
            }
            CleanStep::Remesh { cell, sharp, with, area } => {
                let mut p = json!({ "sharp": sharp });
                if let Some(a) = area {
                    p["area"] = json!(a);
                }
                if let Some(c) = cell {
                    p["cell"] = json!(c);
                }
                if !with.is_empty() {
                    p["with"] = json!(with.iter().map(|w| w.0.clone()).collect::<Vec<_>>());
                }
                p
            }
            CleanStep::RemoveMarkers { radius, centres, fair } => json!({ "radius": radius, "centres": centres, "fair": fair }),
        }
    }

    /// The other scans the step reads (Remesh's `with`).
    pub fn with(&self) -> &[BodyName] {
        match self {
            CleanStep::Remesh { with, .. } => with,
            _ => &[],
        }
    }

    /// Settings a step can be run with: numbers that are numbers, within their tool's range.
    pub fn check(&self) -> crate::Result<()> {
        let bad = |why: &str| Err(crate::DocError::Invalid(format!("a {} step {why}", self.title())));
        match self {
            CleanStep::RemoveFloating { below, .. } if !(0.0..=0.5).contains(below) => bad("needs a share of the largest piece from 0 to 0.5"),
            CleanStep::RemoveFloating { keep, remove, .. } if !points_ok(keep) || !points_ok(remove) => {
                bad("takes at most 10000 points of three finite numbers")
            }
            CleanStep::FillHoles { at, .. } if !points_ok(at) => bad("takes at most 10000 points of three finite numbers"),
            CleanStep::FillHoles { max_edges, .. } if !(3..=5000).contains(max_edges) => bad("closes holes of 3 to 5000 edges"),
            CleanStep::Reduce { area: Some(a), .. } | CleanStep::Denoise { area: Some(a), .. } | CleanStep::Remesh { area: Some(a), .. }
                if !a.ok() || a.count() == 0 =>
            {
                bad("needs an area of the scan's triangles, in order")
            }
            CleanStep::Remesh { area: Some(_), with, .. } if !with.is_empty() => bad("fuses other scans in only over the whole scan, not an area"),
            CleanStep::Reduce { ratio, triangles, max_error, .. } => {
                if ratio.is_some_and(|r| !(r > 0.0 && r <= 1.0)) {
                    bad("keeps a share above 0 and at most 1")
                } else if triangles.is_some_and(|n| n == 0) {
                    bad("keeps at least one triangle")
                } else if ratio.is_some() && triangles.is_some() {
                    bad("keeps a share or a count, not both")
                } else if !(max_error.is_finite() && *max_error >= 0.0) {
                    bad("allows an error that is a length")
                } else {
                    Ok(())
                }
            }
            CleanStep::Denoise { strength: Some(k), .. } if !(0.0..=solidcraft_scan::denoise::MOST_PASSES as f64).contains(k) => {
                bad("smooths 0 to 8 passes")
            }
            CleanStep::Remesh { cell: Some(c), .. } if !(c.is_finite() && *c > 0.0) => bad("needs a cell that is a length above 0"),
            CleanStep::Remesh { with, .. } if with.len() > MAX_WITH => bad("fuses in too many scans"),
            CleanStep::RemoveMarkers { radius, .. } if !(radius.is_finite() && *radius > 0.0) => bad("needs a radius that is a length above 0"),
            CleanStep::RemoveMarkers { centres, .. } if centres.is_empty() || !points_ok(centres) => {
                bad("takes 1 to 10000 centres of three finite numbers")
            }
            _ => Ok(()),
        }
    }
}

/// What a step did, for the command that made it to say.
#[derive(Clone, Debug, PartialEq)]
pub enum Report {
    Floating { pieces: usize, removed: usize, removed_triangles: usize },
    Holes { holes: usize, filled: usize, failed: usize, larger: usize, added_triangles: usize, unfillable_edges: usize },
    Reduce { before: usize, collapses: usize, stopped: &'static str, largest_error: f64 },
    Repair { duplicates: usize, non_manifold: usize, slivers: usize, crossings: usize, crossing_faces: usize, refilled: usize },
    Denoise { strength: f64, suggested: Option<f64>, noise: f64, roughness: f64, rms_moved: f64, max_moved: f64 },
    Remesh { before: usize, cell: f64, new_rms: f64, new_max: f64, scan_rms: f64, scan_p95: f64, uncovered: f64 },
    Markers { markers: usize, removed_triangles: usize, filled: usize, failed: usize },
}

/// Why a step made nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Refused {
    /// The scan needs nothing of this step as it now is: it is left as it is.
    Nothing(String),
    /// The step cannot be done on this scan.
    Failed(String),
}

impl Refused {
    pub fn why(&self) -> &str {
        match self {
            Refused::Nothing(why) | Refused::Failed(why) => why,
        }
    }
}

/// A step done: the scan as it leaves it (a mesh body, uncoloured) and what it did.
#[derive(Clone, Debug)]
pub struct Cleaned {
    pub body: Body,
    pub report: Report,
}

impl Cleaned {
    /// How many triangles the cleaned scan has.
    pub fn triangles(&self) -> usize {
        self.body.triangle_mesh().map_or(0, |(_, t)| t.len())
    }
}

/// The last step worked out: on which scans (held weakly, so the scans are not kept alive by it),
/// with which settings, and what it made.
struct Last {
    scan: Weak<solidcraft_geom::Mesh>,
    with: Vec<Weak<solidcraft_geom::Mesh>>,
    step: CleanStep,
    done: Result<Cleaned, Refused>,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

fn same(kept: &Weak<solidcraft_geom::Mesh>, body: &Body) -> bool {
    match (kept.upgrade(), body.kept_mesh()) {
        (Some(a), Some(b)) => Arc::ptr_eq(&a, &b),
        _ => false,
    }
}

/// Points clicked on a scan, as a step keeps them: few enough, and numbers.
fn points_ok(points: &[[f64; 3]]) -> bool {
    points.len() <= 10_000 && points.iter().flatten().all(|x| x.is_finite())
}

/// The step done on the scan `scan` (a mesh body), fusing in `with` for a Remesh: worked out once
/// for the same scans and settings, however many times it is asked for in a row.
pub fn clean(step: &CleanStep, scan: &Body, with: &[&Body]) -> Result<Cleaned, Refused> {
    if let Ok(last) = LAST.lock()
        && let Some(l) = last.as_ref()
        && l.step == *step
        && same(&l.scan, scan)
        && l.with.len() == with.len()
        && l.with.iter().zip(with).all(|(k, b)| same(k, b))
    {
        return l.done.clone();
    }
    let done = work(step, scan, with);
    if let (Some(kept), Ok(mut last)) = (scan.kept_mesh(), LAST.lock()) {
        *last = Some(Last {
            scan: Arc::downgrade(&kept),
            with: with.iter().filter_map(|b| b.kept_mesh()).map(|m| Arc::downgrade(&m)).collect(),
            step: step.clone(),
            done: done.clone(),
        });
    }
    done
}

/// A step's area as flags on the scan it meets (`None`: the whole scan). Refused when the scan is
/// not the one it was chosen on: a step before it was changed.
fn within(area: Option<&StepArea>, triangles: usize) -> Result<Option<Vec<bool>>, Refused> {
    match area {
        None => Ok(None),
        Some(a) if a.triangles as usize != triangles => Err(Refused::Failed(format!(
            "its area was chosen on the scan with {} triangles; a step before it now leaves {triangles}: choose the area again",
            a.triangles
        ))),
        Some(a) => Ok(Some(a.flags())),
    }
}

fn work(step: &CleanStep, scan: &Body, with: &[&Body]) -> Result<Cleaned, Refused> {
    use solidcraft_scan::{denoise, holes, pieces, reduce, remesh, repair};
    let (positions, triangles) = scan.triangle_mesh().ok_or_else(|| Refused::Failed("the scan is not a mesh body".into()))?;
    let on = Scan::new(positions, triangles);
    let made = |p: &[Vec3], t: &[[u32; 3]], report: Report| -> Result<Cleaned, Refused> {
        let body = solidcraft_kernel::mesh_body(p, t).map_err(|e| Refused::Failed(format!("the cleaned scan is not a mesh: {e}")))?;
        Ok(Cleaned { body, report })
    };
    match step {
        CleanStep::RemoveFloating { below, keep, remove } => {
            let found = pieces::pieces(&on);
            let mut floating = found.floating(*below);
            // The pieces clicked: kept or taken off whatever their size.
            let piece = |at: &[f64; 3]| {
                let t = solidcraft_scan::base::nearest_triangle(&on, Vec3::new(at[0], at[1], at[2]))?;
                found.of.get(t).copied().filter(|&k| k != u32::MAX).map(|k| k as usize)
            };
            for (points, goes) in [(keep, false), (remove, true)] {
                for at in points {
                    if let Some(f) = piece(at).and_then(|k| floating.get_mut(k)) {
                        *f = goes;
                    }
                }
            }
            let removed = floating.iter().filter(|f| **f).count();
            if removed == 0 {
                return Err(Refused::Nothing(format!(
                    "no piece floats: all {} are at least {:.1} % of the largest",
                    found.sizes.len(),
                    100.0 * below
                )));
            }
            let (p, t) = pieces::keeping(&on, |t| found.of.get(t).is_some_and(|&k| k != u32::MAX && !floating[k as usize]));
            let removed_triangles = found.sizes.iter().zip(&floating).filter(|(_, f)| **f).map(|(n, _)| n).sum();
            made(&p, &t, Report::Floating { pieces: found.sizes.len(), removed, removed_triangles })
        }
        CleanStep::FillHoles { at, fair, .. } if !at.is_empty() => {
            let found = holes::survey(&on);
            let points: Vec<Vec3> = at.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect();
            let chosen = found.chosen(positions, &points);
            let count = found.rings.iter().filter(|r| !r.rim).count();
            let done = holes::fill_rings(&on, &chosen, *fair)
                .ok_or_else(|| Refused::Failed("no hole could be closed: each ring folds or crosses the scan".into()))?;
            let report = Report::Holes {
                holes: count,
                filled: done.filled,
                failed: done.failed,
                larger: 0,
                added_triangles: done.added_triangles,
                unfillable_edges: found.unfillable_edges,
            };
            made(&done.positions, &done.triangles, report)
        }
        CleanStep::FillHoles { max_edges, fair, .. } => {
            let limit = *max_edges as usize;
            let found = holes::survey(&on);
            let count = found.rings.iter().filter(|r| !r.rim).count();
            let within = found.holes(limit).count();
            if within == 0 {
                return Err(Refused::Nothing(if count == 0 {
                    let pinched = if found.unfillable_edges > 0 {
                        format!(" ({} open edges pinch or dead-end, so they make no ring to close)", found.unfillable_edges)
                    } else {
                        String::new()
                    };
                    match found.rings.len() {
                        0 if found.unfillable_edges == 0 => "the scan has no holes: it is closed".to_owned(),
                        0 => format!("the scan has no holes to close{pinched}"),
                        _ => format!("the scan has no holes to close: its only open ring is its outer rim{pinched}"),
                    }
                } else {
                    format!(
                        "no hole has {limit} edges or fewer: the smallest has {}",
                        found.rings.iter().filter(|r| !r.rim).map(|r| r.points.len()).min().unwrap_or(0)
                    )
                }));
            }
            let done = holes::fill(&on, &found, limit, *fair)
                .ok_or_else(|| Refused::Failed("no hole could be closed: each ring folds or crosses the scan".into()))?;
            let report = Report::Holes {
                holes: count,
                filled: done.filled,
                failed: done.failed,
                larger: count - within,
                added_triangles: done.added_triangles,
                unfillable_edges: found.unfillable_edges,
            };
            made(&done.positions, &done.triangles, report)
        }
        CleanStep::Reduce { ratio, triangles: count, max_error, keep_rim, area } => {
            let before = on.triangle_count();
            let flags = within(area.as_ref(), before)?;
            // A share is of the area's triangles; the rest are kept as they are.
            let (inside, outside) = flags.as_ref().map_or((before, 0), |f| {
                let n = f.iter().filter(|x| **x).count();
                (n, before - n)
            });
            let target = match (ratio, count) {
                (Some(r), _) => outside + ((r * inside as f64).round() as usize).max(1),
                (_, Some(n)) => usize::try_from(*n).unwrap_or(usize::MAX),
                _ if *max_error > 0.0 => 0,
                _ => outside + (inside / 2).max(1),
            };
            if target >= before && *max_error == 0.0 {
                return Err(Refused::Nothing(format!("the scan has {before} triangles already: ask for fewer")));
            }
            let reduced = reduce::reduce_within(&on, reduce::Settings { target, max_error: *max_error, keep_rim: *keep_rim }, flags.as_deref())
                .ok_or_else(|| Refused::Failed("the scan has no triangles to reduce".into()))?;
            if reduced.collapses == 0 {
                return Err(Refused::Nothing("nothing could be collapsed within the error allowed".into()));
            }
            let report =
                Report::Reduce { before, collapses: reduced.collapses, stopped: reduced.stopped.name(), largest_error: reduced.largest_error };
            made(&reduced.positions, &reduced.triangles, report)
        }
        CleanStep::RemoveMarkers { radius, centres, fair } => {
            let at: Vec<Vec3> = centres.iter().map(|c| Vec3::new(c[0], c[1], c[2])).collect();
            let done = solidcraft_scan::markers::remove(&on, &at, *radius, *fair)
                .ok_or_else(|| Refused::Nothing("no marker is on the scan any more: nothing to take off".into()))?;
            let report = Report::Markers { markers: at.len(), removed_triangles: done.removed, filled: done.filled, failed: done.failed };
            made(&done.positions, &done.triangles, report)
        }
        CleanStep::Repair { crossings } => {
            let fixed = repair::repair(&on, *crossings);
            if !fixed.changed() {
                return Err(Refused::Nothing(
                    "the scan needs no repair: no copied triangles, no edge of three or more faces, no slivers, no faces passing through others"
                        .into(),
                ));
            }
            let report = Report::Repair {
                duplicates: fixed.duplicates,
                non_manifold: fixed.non_manifold,
                slivers: fixed.slivers,
                crossings: fixed.crossings,
                crossing_faces: fixed.crossing_faces,
                refilled: fixed.refilled,
            };
            made(&fixed.positions, &fixed.triangles, report)
        }
        CleanStep::Denoise { strength, area } => {
            let flags = within(area.as_ref(), on.triangle_count())?;
            let done = denoise::denoise_within(&on, *strength, flags.as_deref())
                .ok_or_else(|| Refused::Failed("the scan has no triangles to smooth".into()))?;
            if !(done.strength > 0.0) {
                return Err(Refused::Nothing(match strength {
                    Some(_) => "a `strength` of 0 smooths nothing: give 0.05 to 8 passes".to_owned(),
                    None => format!(
                        "smoothing would not lower the scan's error: it is as smooth as it can be read (noise {:.4} mm); give a `strength` to smooth it anyway",
                        done.noise
                    ),
                }));
            }
            let report = Report::Denoise {
                strength: done.strength,
                suggested: done.suggested,
                noise: done.noise,
                roughness: done.roughness,
                rms_moved: done.rms_moved,
                max_moved: done.max_moved,
            };
            made(&done.positions, triangles, report)
        }
        CleanStep::Remesh { cell, sharp, area, .. } => {
            let flags = within(area.as_ref(), on.triangle_count())?;
            let cell = match cell {
                Some(c) => *c,
                None => {
                    let edge = remesh::typical_edge(&on);
                    if !(edge > 0.0) {
                        return Err(Refused::Failed("the scan has no edges to remesh".into()));
                    }
                    edge
                }
            };
            if let Some(flags) = flags {
                let done = solidcraft_scan::remesh_area::remesh_area(&on, &flags, cell, *sharp).map_err(|e| Refused::Failed(e.to_string()))?;
                let report = Report::Remesh {
                    before: on.triangle_count(),
                    cell,
                    new_rms: done.new_rms,
                    new_max: done.new_max,
                    scan_rms: done.scan_rms,
                    scan_p95: done.scan_p95,
                    uncovered: done.uncovered,
                };
                return made(&done.positions, &done.triangles, report);
            }
            let mut sources = vec![on];
            for b in with {
                let (p, t) = b.triangle_mesh().ok_or_else(|| Refused::Failed("a scan fused in is not a mesh body".into()))?;
                sources.push(Scan::new(p, t));
            }
            let done = remesh::remesh(&sources, &remesh::Settings { cell, sharp: *sharp }).map_err(|e| Refused::Failed(e.to_string()))?;
            let report = Report::Remesh {
                before: on.triangle_count(),
                cell,
                new_rms: done.new_rms,
                new_max: done.new_max,
                scan_rms: done.scan_rms,
                scan_p95: done.scan_p95,
                uncovered: done.uncovered,
            };
            made(&done.positions, &done.triangles, report)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles apart: a square of two and a lone triangle far off.
    fn scan() -> Body {
        let p = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(10.0, 10.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(50.0, 0.0, 0.0),
            Vec3::new(51.0, 0.0, 0.0),
            Vec3::new(50.0, 1.0, 0.0),
        ];
        let t = [[0, 1, 2], [0, 2, 3], [4, 5, 6]];
        solidcraft_kernel::mesh_body(&p, &t).unwrap()
    }

    #[test]
    fn a_step_round_trips_and_is_checked() {
        let steps = [
            CleanStep::RemoveFloating { below: 0.6, keep: Vec::new(), remove: Vec::new() },
            CleanStep::FillHoles { max_edges: 2, fair: true, at: Vec::new() },
            CleanStep::Reduce { ratio: Some(0.5), triangles: Some(3), max_error: 0.0, keep_rim: true, area: None },
            CleanStep::Denoise { strength: Some(9.0), area: None },
            // Runs out of order, past the scan, or empty.
            CleanStep::Denoise { strength: None, area: Some(StepArea { triangles: 10, runs: vec![[5, 2], [1, 1]] }) },
            CleanStep::Denoise { strength: None, area: Some(StepArea { triangles: 10, runs: vec![[8, 5]] }) },
            CleanStep::Reduce {
                ratio: None,
                triangles: None,
                max_error: 0.1,
                keep_rim: true,
                area: Some(StepArea { triangles: 10, runs: Vec::new() }),
            },
            CleanStep::Remesh { cell: Some(0.0), sharp: false, with: Vec::new(), area: None },
            CleanStep::Remesh { cell: None, sharp: false, with: vec!["other".into()], area: Some(StepArea { triangles: 10, runs: vec![[0, 4]] }) },
        ];
        for s in &steps {
            assert!(s.check().is_err(), "{s:?}");
        }
        let fine = CleanStep::Reduce { ratio: Some(0.5), triangles: None, max_error: 0.1, keep_rim: false, area: None };
        fine.check().unwrap();
        let text = serde_json::to_string(&fine).unwrap();
        assert_eq!(text, r#"{"op":"reduce","ratio":0.5,"max_error":0.1,"keep_rim":false}"#);
        assert_eq!(serde_json::from_str::<CleanStep>(&text).unwrap(), fine);
    }

    /// An area keeps its triangles as runs and gives them back; a step with one round-trips, and a
    /// step saved before areas (none) still reads, as the whole scan.
    #[test]
    fn an_area_round_trips_as_runs() {
        let chosen = [false, true, true, true, false, false, true, false];
        let area = StepArea::of(&chosen);
        assert_eq!(area, StepArea { triangles: 8, runs: vec![[1, 3], [6, 1]] });
        assert_eq!(area.flags(), chosen);
        assert_eq!(area.count(), 4);
        let step = CleanStep::Denoise { strength: Some(2.0), area: Some(area) };
        step.check().unwrap();
        let text = serde_json::to_string(&step).unwrap();
        assert_eq!(text, r#"{"op":"denoise","strength":2.0,"area":{"triangles":8,"runs":[[1,3],[6,1]]}}"#);
        assert_eq!(serde_json::from_str::<CleanStep>(&text).unwrap(), step);
        assert_eq!(
            serde_json::from_str::<CleanStep>(r#"{"op":"denoise","strength":2.0}"#).unwrap(),
            CleanStep::Denoise { strength: Some(2.0), area: None }
        );
        // On a scan with other triangles than it was chosen on, the step says so.
        let other = CleanStep::Denoise { strength: Some(2.0), area: Some(StepArea { triangles: 99, runs: vec![[0, 1]] }) };
        assert!(matches!(clean(&other, &scan(), &[]), Err(Refused::Failed(why)) if why.contains("choose the area again")));
    }

    #[test]
    fn floating_pieces_go_once_and_a_second_time_there_is_nothing() {
        let step = CleanStep::RemoveFloating { below: 0.6, keep: Vec::new(), remove: Vec::new() };
        let scan = scan();
        let done = clean(&step, &scan, &[]).unwrap();
        assert_eq!(done.triangles(), 2);
        assert_eq!(done.report, Report::Floating { pieces: 2, removed: 1, removed_triangles: 1 });
        // Asked again for the same scan: the same result, not worked out again.
        let again = clean(&step, &scan, &[]).unwrap();
        assert!(Arc::ptr_eq(&again.body.kept_mesh().unwrap(), &done.body.kept_mesh().unwrap()));
        assert!(matches!(clean(&step, &done.body, &[]), Err(Refused::Nothing(_))));
    }
}
