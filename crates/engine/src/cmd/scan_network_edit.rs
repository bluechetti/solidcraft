//! A Free Form's network edited as a whole by `scan.edit_surface` (`smooth`, `delete_face`, `weld`,
//! `keep_half`), and what the design keeps of a Free Form besides its faces (`Document.free_forms`):
//! the shared edges creased or held smooth by hand where the faces' angle reads them the other way,
//! and the plane Symmetry keeps it a mirror image in. Every edit of a network reads them back
//! ([`joins_for`], [`wanted_symmetry`]) and keeps them again as they are after it ([`keep`]), in
//! the same undo step.

use serde_json::Value;
use solidcore_record::Vec3;
use solidcraft_doc::free_form::{FreeFormKeep, KeptPlane, MAX_FREE_FORMS};
use solidcraft_geom::Vec3Geom as _;
use solidcraft_geom::bspline::NurbsSurface;
use solidcraft_scan::network::{self, Joins};
use solidcraft_scan::symmetric::{self, MirrorPlane, Pairs};

use super::length;
use super::surface::{EditableSurface, default_reach, faces_reply, shade, write_faces};
use crate::params::{bad, str_};
use crate::{Result, Session};

/// What the design keeps of a Free Form besides its faces, where the Free Form is now (moved as
/// later features moved it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kept {
    pub creases: Vec<(Vec3, Vec3)>,
    pub smooths: Vec<(Vec3, Vec3)>,
    pub symmetry: Option<MirrorPlane>,
}

fn vec3(p: [f64; 3]) -> Vec3 {
    Vec3::new(p[0], p[1], p[2])
}

/// What the design keeps of the Free Form `target`, where it is now.
pub fn kept(s: &Session, target: &EditableSurface) -> Kept {
    let Some(k) = s.doc.free_forms.iter().find(|k| k.feature == target.feature) else { return Kept::default() };
    let now = |p: [f64; 3]| target.placed.as_ref().map_or(vec3(p), |r| r.apply(vec3(p)));
    let edges = |list: &[[[f64; 3]; 2]]| list.iter().map(|[a, b]| (now(*a), now(*b))).collect();
    let symmetry = k.symmetry.and_then(|pl| {
        let normal = target.placed.as_ref().map_or(vec3(pl.normal), |r| r.turned(vec3(pl.normal)));
        MirrorPlane::new(now(pl.point), normal)
    });
    Kept { creases: edges(&k.creases), smooths: edges(&k.smooths), symmetry }
}

/// How `faces` (the Free Form `target`'s, as they are or are about to be) join, with the edges the
/// design keeps creased or smooth held so.
pub fn joins_for(s: &Session, target: &EditableSurface, faces: &[NurbsSurface], cmd: &str) -> Result<Joins> {
    let k = kept(s, target);
    network::joins_keeping(faces, &k.creases, &k.smooths, network::ends_tolerance(faces)).map_err(|e| bad(cmd, e.to_string()))
}

/// Keeps in the design what `joins` holds by hand on `faces` (the Free Form `target`'s as written)
/// and its symmetry plane: `symmetry` `None` leaves the plane kept as it is, `Some(None)` drops it.
/// The design is touched only when what it keeps changes.
pub fn keep(s: &mut Session, target: &EditableSurface, faces: &[NurbsSurface], joins: &Joins, symmetry: Option<Option<MirrorPlane>>) {
    let (creases, smooths) = if faces.len() > 1 { network::held_by_hand(faces, joins) } else { (Vec::new(), Vec::new()) };
    let back = target.placed.as_ref().map(|r| r.inverse());
    let in_file = |p: Vec3| {
        let q = back.as_ref().map_or(p, |r| r.apply(p));
        [q.x, q.y, q.z]
    };
    let edges = |list: Vec<(Vec3, Vec3)>| list.into_iter().map(|(a, b)| [in_file(a), in_file(b)]).collect::<Vec<_>>();
    let at = s.doc.free_forms.iter().position(|k| k.feature == target.feature);
    let was = at.and_then(|i| s.doc.free_forms[i].symmetry);
    let symmetry = match symmetry {
        None => was,
        Some(None) => None,
        Some(Some(pl)) => {
            let n = back.as_ref().map_or(pl.normal, |r| r.turned(pl.normal));
            let plane = KeptPlane { point: in_file(pl.point), normal: [n.x, n.y, n.z] };
            // The plane kept already, named another way (another point on it, the normal turned
            // round): kept as it was, so the design does not change.
            match was {
                Some(w) if same_plane(&w, &plane) => Some(w),
                _ => Some(plane),
            }
        }
    };
    let entry = FreeFormKeep { feature: target.feature, creases: edges(creases), smooths: edges(smooths), symmetry };
    match (at, entry.is_empty()) {
        (Some(i), true) => {
            s.doc_mut().free_forms.remove(i);
        }
        (Some(i), false) if s.doc.free_forms[i] != entry => s.doc_mut().free_forms[i] = entry,
        (None, false) if s.doc.free_forms.len() < MAX_FREE_FORMS => s.doc_mut().free_forms.push(entry),
        _ => {}
    }
}

/// Whether two kept planes are one: facing the same way or the other, through the same place.
fn same_plane(a: &KeptPlane, b: &KeptPlane) -> bool {
    match (MirrorPlane::new(vec3(a.point), vec3(a.normal)), MirrorPlane::new(vec3(b.point), vec3(b.normal))) {
        (Some(p), Some(q)) => p.normal.dot(q.normal).abs() > 1.0 - 1e-9 && p.side(q.point).abs() < 1e-6,
        _ => false,
    }
}

/// The plane an edit keeps the Free Form a mirror image in: `symmetry` as given (`none` for none),
/// else the one the design keeps. Also whether it was given.
pub fn wanted_symmetry(s: &Session, target: &EditableSurface, p: &Value, cmd: &str) -> Result<(Option<MirrorPlane>, bool)> {
    match p.get("symmetry").filter(|v| !v.is_null()) {
        Some(Value::String(none)) if none.trim().eq_ignore_ascii_case("none") => Ok((None, true)),
        Some(v) => Ok((Some(super::free_form::mirror_plane_param(s, Some(v), cmd)?), true)),
        None => Ok((kept(s, target).symmetry, false)),
    }
}

/// The pairs of `faces` in `plane`, when they are a mirror image in it.
pub fn pairs_in(faces: &[NurbsSurface], joins: &Joins, plane: MirrorPlane) -> Option<Pairs> {
    let tolerance = symmetric::pairing_tolerance(faces, faces.iter().map(default_reach).fold(1.0, f64::max));
    symmetric::pairs_of(faces, joins, plane, tolerance).ok()
}

/// The pairs an edit keeps the Free Form a mirror image by, and what the design should keep of
/// its plane afterwards: a plane given that the Free Form is not a mirror image in is refused; a
/// kept one it no longer is (an edit elsewhere broke it) is let go.
pub fn edit_pairs(
    s: &Session,
    target: &EditableSurface,
    joins: &Joins,
    p: &Value,
    cmd: &str,
) -> Result<(Option<Pairs>, Option<Option<MirrorPlane>>)> {
    let (plane, given) = wanted_symmetry(s, target, p, cmd)?;
    let Some(plane) = plane else { return Ok((None, given.then_some(None))) };
    match pairs_in(&target.faces, joins, plane) {
        Some(pairs) => Ok((Some(pairs), Some(Some(plane)))),
        None if given => Err(bad(cmd, format!("`symmetry`: {}", symmetric::MirrorError::NotMirror))),
        None => Ok((None, Some(None))),
    }
}

/// The scan points under `faces` (the scan `scan`, or the selected or only one), for fitting.
fn scan_points(s: &Session, p: &Value, faces: &[NurbsSurface], cmd: &str) -> Result<Vec<Vec3>> {
    let (_, sh) = shade(s, str_(p, "scan"), faces, cmd)?;
    Ok(sh.points.iter().map(|u| u.at).collect())
}

/// A whole number under `key` of `v` within `range`, or `default`.
fn count(v: &Value, key: &str, range: std::ops::RangeInclusive<u64>, default: u64, cmd: &str, what: &str) -> Result<u64> {
    match v.get(key).filter(|x| !x.is_null()) {
        None => Ok(default),
        Some(x) => x
            .as_u64()
            .filter(|n| range.contains(n))
            .ok_or_else(|| bad(cmd, format!("`{what}` must be a whole number, {} to {}", range.start(), range.end()))),
    }
}

/// A place `[x, y, z]` under `key` of `v`.
fn place(v: &Value, key: &str, cmd: &str, what: &str) -> Result<Vec3> {
    v.get(key).and_then(crate::params::vec3).ok_or_else(|| bad(cmd, format!("`{what}` must be a point [x, y, z] of numbers")))
}

/// Refused for a surface of one face: `what` works on a Free Form of several faces.
pub fn need_free_form(target: &EditableSurface, what: &str, cmd: &str) -> Result<()> {
    if target.is_network() {
        return Ok(());
    }
    Err(bad(cmd, format!("`{}` is one face: {what} works on a Free Form of several faces", target.body)))
}

/// The network smoothed (Edit Surface's Smooth): every point but its own corners and the corners
/// where faces meet unevenly straightens its rows and columns, `passes` times, kept near the scan
/// (`snap`, the default) or where the surface is.
pub(super) fn smoothed(s: &mut Session, target: &EditableSurface, v: &Value, p: &Value, cmd: &str) -> Result<Value> {
    if !v.is_object() && v.as_bool() != Some(true) {
        return Err(bad(cmd, "`smooth` must be true or {strength?, passes?, snap?}"));
    }
    let strength = match v.get("strength").filter(|x| !x.is_null()) {
        None => 0.5,
        Some(x) => x
            .as_f64()
            .filter(|f| f.is_finite() && *f > 0.0 && *f <= 1.0)
            .ok_or_else(|| bad(cmd, "`smooth.strength` must be above 0 and at most 1"))?,
    };
    let passes = count(v, "passes", 1..=20, 1, cmd, "smooth.passes")?;
    let snap = match v.get("snap").filter(|x| !x.is_null()) {
        None => true,
        Some(x) => x.as_bool().ok_or_else(|| bad(cmd, "`smooth.snap` must be true or false"))?,
    };
    let reach = length(s, p, "reach", target.faces.iter().map(default_reach).fold(1.0, f64::max), cmd)?;
    let joins = joins_for(s, target, &target.faces, cmd)?;
    let (pairs, symmetry) = edit_pairs(s, target, &joins, p, cmd)?;
    let scan = if snap { scan_points(s, p, &target.faces, cmd)? } else { Vec::new() };
    let all = vec![true; target.faces.len()];
    let mut faces = target.faces.clone();
    for _ in 0..passes {
        let goals = if scan.is_empty() { Vec::new() } else { network::feet(&faces, &scan, reach, &all) };
        faces = network::smoothed(&faces, &joins, &goals, strength)
            .ok_or_else(|| bad(cmd, "nothing on the surface may move: its own corners stay where they are"))?;
        if let Some(pairs) = &pairs {
            faces = pairs.kept(&faces, &joins).unwrap_or(faces);
        }
    }
    write_faces(s, target, &faces, cmd)?;
    keep(s, target, &faces, &joins, symmetry);
    faces_reply(s, target, &faces, &joins, 0, cmd)
}

/// The face of `faces` nearest `at`: the one whose surface comes closest to it.
fn face_near(faces: &[NurbsSurface], at: Vec3) -> Option<(usize, f64)> {
    faces
        .iter()
        .enumerate()
        .filter_map(|(i, f)| solidcraft_scan::surface_edit::foot(f, at).map(|foot| (i, foot.point.dist(at))))
        .filter(|(_, d)| d.is_finite())
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// A face of the network taken away (Edit Surface's Delete Face): the face nearest `at`, and with
/// Symmetry its mirror image too. The last face, and one whose going leaves the rest in two
/// pieces, stay.
pub(super) fn deleted_face(s: &mut Session, target: &EditableSurface, v: &Value, p: &Value, cmd: &str) -> Result<Value> {
    need_free_form(target, "Delete Face", cmd)?;
    let at = place(v, "at", cmd, "delete_face.at")?;
    let reach = length(s, p, "reach", target.faces.iter().map(default_reach).fold(1.0, f64::max), cmd)?;
    let (face, off) = face_near(&target.faces, at).ok_or_else(|| bad(cmd, "the Free Form has no face there"))?;
    if off > reach {
        return Err(bad(cmd, format!("`delete_face.at` is {off:.2} mm from the nearest face, more than the reach ({reach:.2} mm)")));
    }
    let joins = joins_for(s, target, &target.faces, cmd)?;
    let (pairs, symmetry) = edit_pairs(s, target, &joins, p, cmd)?;
    let mut gone = vec![face];
    if let Some(pairs) = &pairs
        && let Some(twin) = symmetric::twin_face(&target.faces, pairs.plane, face, 1e-6).filter(|t| *t != face)
    {
        gone.push(twin);
    }
    gone.sort_unstable();
    let made = gone.iter().rev().try_fold(target.faces.clone(), |faces, &f| network::without_face(&faces, f)).map_err(|e| bad(cmd, e.to_string()))?;
    write_faces(s, target, &made, cmd)?;
    let joins = joins_for(s, target, &made, cmd)?;
    keep(s, target, &made, &joins, symmetry);
    faces_reply(s, target, &made, &joins, 0, cmd)
}

/// The corner on the network's edge nearest `near` (other than `but`), and how far it is.
fn edge_corner(faces: &[NurbsSurface], joins: &Joins, near: Vec3, but: Option<usize>) -> Option<(usize, f64)> {
    let places = network::network_points(faces, joins);
    joins
        .corners
        .iter()
        .filter(|c| c.border && Some(c.point) != but)
        .filter_map(|c| places.get(c.point).map(|p| (c.point, p.dist(near))))
        .min_by(|a, b| a.1.total_cmp(&b.1))
}

/// `faces` with the corner on the network's edge nearest `at` dropped onto the one nearest `onto`
/// ([`network::welded`]): two open sides whose ends then meet are sewn into a shared edge, held
/// tangent where the faces go on from each other within [`network::CONTINUES_DEGREES`] (else left a
/// crease), the points on the surface near it held where they are (they lie on the scan already:
/// refitting them to it slid the net about). The faces, the sides sewn, and how the faces join. As
/// Edit Surface does when a corner is dragged onto another.
pub fn corner_welded(
    faces: &[NurbsSurface],
    joins: &Joins,
    point: usize,
    onto: Vec3,
    reach: f64,
    keep_creases: &[(Vec3, Vec3)],
    keep_smooths: &[(Vec3, Vec3)],
) -> Option<(Vec<NurbsSurface>, usize, Joins, Vec<(Vec3, Vec3)>)> {
    let made = network::welded(faces, joins, point, onto)?;
    let mut faces = made.faces;
    let mut smooths: Vec<(Vec3, Vec3)> = keep_smooths.to_vec();
    let tolerance = network::ends_tolerance(&faces);
    let mut joins = network::joins_keeping(&faces, keep_creases, &smooths, tolerance).ok()?;
    for &(a, b) in &made.sewn {
        let Some(seam) = network::seam_between(&faces, &joins, a, b, tolerance) else { continue };
        if network::seam_angle(&faces, &joins.seams[seam]) > network::CONTINUES_DEGREES {
            continue;
        }
        joins = network::with_tangency(&faces, &joins, seam);
        let held = vec![false; joins.len()];
        if let Some(next) = network::refit_near_seam(&faces, &joins, seam, &[], reach, &held) {
            faces = next;
        }
        smooths.push((a, b));
    }
    Some((faces, made.sewn.len(), joins, smooths))
}

/// A corner on the network's edge welded to another (Edit Surface's drop of a corner on a corner):
/// the corner nearest `at` put on the one nearest `onto`, each within the reach; open sides whose
/// ends then meet sewn into one shared edge. With Symmetry the mirror image is welded too.
pub(super) fn welded_corner(s: &mut Session, target: &EditableSurface, v: &Value, p: &Value, cmd: &str) -> Result<Value> {
    let at = place(v, "at", cmd, "weld.at")?;
    let onto = place(v, "onto", cmd, "weld.onto")?;
    let reach = length(s, p, "reach", target.faces.iter().map(default_reach).fold(1.0, f64::max), cmd)?;
    let joins = joins_for(s, target, &target.faces, cmd)?;
    let (pairs, symmetry) = edit_pairs(s, target, &joins, p, cmd)?;
    let k = kept(s, target);
    // One weld, of `from` near `at` onto the corner near `onto`, on `faces` as they are.
    let weld_one =
        |faces: &[NurbsSurface], from: Vec3, to: Vec3, smooths: &[(Vec3, Vec3)]| -> Result<(Vec<NurbsSurface>, usize, Joins, Vec<(Vec3, Vec3)>)> {
            let joins = network::joins_keeping(faces, &k.creases, smooths, network::ends_tolerance(faces)).map_err(|e| bad(cmd, e.to_string()))?;
            let (point, off) = edge_corner(faces, &joins, from, None).ok_or_else(|| bad(cmd, "the Free Form has no corner on its edge to weld"))?;
            if off > reach {
                return Err(bad(
                    cmd,
                    format!("`weld.at` is {off:.2} mm from the nearest corner on the Free Form's edge, more than the reach ({reach:.2} mm)"),
                ));
            }
            let (other, off) =
                edge_corner(faces, &joins, to, Some(point)).ok_or_else(|| bad(cmd, "the Free Form has no other corner on its edge to weld onto"))?;
            if off > reach {
                return Err(bad(
                    cmd,
                    format!("`weld.onto` is {off:.2} mm from the nearest other corner on the Free Form's edge, more than the reach ({reach:.2} mm)"),
                ));
            }
            let places = network::network_points(faces, &joins);
            corner_welded(faces, &joins, point, places[other], reach, &k.creases, smooths).ok_or_else(|| bad(cmd, "the corners could not be welded"))
        };
    let (mut faces, mut sewn, mut joins, mut smooths) = weld_one(&target.faces, at, onto, &k.smooths)?;
    if let Some(pairs) = &pairs {
        let (ra, ro) = (pairs.plane.reflect(at), pairs.plane.reflect(onto));
        // A corner on the plane is its own mirror image: welded once.
        if ra.dist(at) > reach || ro.dist(onto) > reach {
            let (f, n, j, both) = weld_one(&faces, ra, ro, &smooths)?;
            faces = symmetric::made_exact(&f, pairs.plane, reach).unwrap_or(f);
            sewn += n;
            smooths = both;
            joins = network::joins_keeping(&faces, &k.creases, &smooths, network::ends_tolerance(&faces)).unwrap_or(j);
        }
    }
    write_faces(s, target, &faces, cmd)?;
    let keep_symmetry = match (&pairs, symmetry) {
        (Some(p), plane) if pairs_in(&faces, &joins, p.plane).is_some() => plane,
        (Some(_), _) => Some(None),
        (None, plane) => plane,
    };
    keep(s, target, &faces, &joins, keep_symmetry);
    let mut reply = faces_reply(s, target, &faces, &joins, 1, cmd)?;
    if let Some(r) = reply.as_object_mut() {
        r.insert("sewn".into(), Value::from(sewn));
    }
    Ok(reply)
}

/// Keep One Half (Symmetry turned off): the faces on the far side of the plane from `side` taken
/// away; the plane the design keeps, or `plane`.
pub(super) fn kept_half(s: &mut Session, target: &EditableSurface, v: &Value, cmd: &str) -> Result<Value> {
    need_free_form(target, "Keep One Half", cmd)?;
    let side = place(v, "side", cmd, "keep_half.side")?;
    let plane = match v.get("plane").filter(|x| !x.is_null()) {
        Some(pl) => super::free_form::mirror_plane_param(s, Some(pl), cmd)?,
        None => {
            kept(s, target).symmetry.ok_or_else(|| bad(cmd, "`keep_half` needs `plane`: the design keeps no symmetry plane for this Free Form"))?
        }
    };
    let made = symmetric::half_kept(&target.faces, plane, side).map_err(|e| bad(cmd, format!("`keep_half`: {e}")))?;
    write_faces(s, target, &made, cmd)?;
    let joins = joins_for(s, target, &made, cmd)?;
    keep(s, target, &made, &joins, Some(None));
    faces_reply(s, target, &made, &joins, 0, cmd)
}

/// Edges given as `[[x, y, z], [x, y, z]]` pairs under `key` of `v` (`kept_edges.creases`, …).
pub fn edge_list(v: &Value, key: &str, cmd: &str) -> Result<Vec<(Vec3, Vec3)>> {
    let Some(list) = v.get(key).filter(|x| !x.is_null()) else { return Ok(Vec::new()) };
    let list = list
        .as_array()
        .filter(|a| a.len() <= solidcraft_doc::free_form::MAX_EDGES)
        .ok_or_else(|| bad(cmd, format!("`kept_edges.{key}` must list edges, each its two ends [[x, y, z], [x, y, z]]")))?;
    list.iter()
        .enumerate()
        .map(|(k, e)| {
            let ends = e.as_array().filter(|a| a.len() == 2);
            match ends.map(|a| (crate::params::vec3(&a[0]), crate::params::vec3(&a[1]))) {
                Some((Some(a), Some(b))) => Ok((a, b)),
                _ => Err(bad(cmd, format!("`kept_edges.{key}[{k}]` must be two points [x, y, z] of numbers"))),
            }
        })
        .collect()
}
