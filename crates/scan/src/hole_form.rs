//! Holes as a drill and a cutter make them, read off the shapes Find All found: a bore with what
//! sits on its axis with it. A wider bore at its mouth is a counterbore, a cone at its mouth a
//! countersink, a cone at its inner end the drill's point (a blind hole), a flat face there a
//! flat bottom; a bore the size of a tap drill is a tapped hole. Such a hole is built as one Hole
//! feature (Hole It) instead of being extruded into a prism's profile, whose slabs a counterbore's
//! step or a blind hole's end would cut it into.
//!
//! Sizes are read as designed: a diameter or a depth that lies within a share of the scan's
//! tolerance of a round size (a whole millimetre, a half, a tenth) is that size, an angle within
//! a few degrees of a drill's or a countersink's usual angle is that angle.

use std::collections::BTreeSet;

use solidcore_record::Vec3;
use solidcraft_geom::prelude::*;

use crate::fit::Shape;
use crate::reconstruct::Face;

/// Directions this close are one axis (a cosine: about 2°).
const SAME_DIRECTION: f64 = 0.9994;
/// Axes this far apart, as a share of the bore's radius (and at least this share of a millimetre),
/// are one axis.
const SAME_AXIS: f64 = 0.2;
/// A size within this share of the scan's tolerance of a round size is that size.
const SNAP_SHARE: f64 = 0.25;
/// An angle within this of a usual one (radians, about 3°) is that one.
const SNAP_ANGLE: f64 = 0.0524;
/// The angles a drill's point or a countersink is usually ground to (degrees, the full angle).
const USUAL_ANGLES: [f64; 7] = [60.0, 82.0, 90.0, 100.0, 118.0, 120.0, 140.0];
/// A drill point is this wide at least and at most (degrees, the full angle).
const TIP_ANGLES: (f64, f64) = (80.0, 150.0);
/// ISO metric coarse threads and the tap drill each is drilled with (mm).
const TAP_DRILLS: [(&str, f64); 10] =
    [("M1.6", 1.25), ("M2", 1.6), ("M2.5", 2.05), ("M3", 2.5), ("M4", 3.3), ("M5", 4.2), ("M6", 5.0), ("M8", 6.8), ("M10", 8.5), ("M12", 10.2)];
/// A drilled hole's bore is no wider than this share of the part's size: a wider round hollow is
/// a recess or a bore the part's profile holds, not a hole a drill makes.
const MOST_BORE_SHARE: f64 = 0.06;
/// A counterbore or a countersink is no wider than this many times its bore (a screw's head).
const MOST_HEAD: f64 = 2.5;
/// A flat-bottomed blind hole is at least this many diameters deep: a shallower one is a recess.
const LEAST_FLAT_DEPTH: f64 = 0.75;
/// A bore within this share of a tap drill's diameter of it (and within [`SNAP_SHARE`] of the
/// tolerance) is drilled for that thread.
const TAP_SHARE: f64 = 0.01;

/// A hole as one Hole feature makes it.
#[derive(Clone, Debug, PartialEq)]
pub struct Formed {
    /// The bore, as an index into the faces, and its radius.
    pub bore: usize,
    pub radius: f64,
    /// On the axis at the mouth (where the hole opens), and the unit direction into the part.
    pub mouth: Vec3,
    pub inward: Vec3,
    /// On the axis where the bore's wall ends inside (the far side, for a hole through).
    pub bottom: Vec3,
    /// A wider bore at the mouth: its radius and how deep it goes from the mouth.
    pub counterbore: Option<(f64, f64)>,
    /// A cone at the mouth: its radius at the mouth and its full angle (radians).
    pub countersink: Option<(f64, f64)>,
    /// A blind hole: how deep the bore goes from the mouth to where its wall ends (the drill
    /// point's shoulder); none for a hole through.
    pub depth: Option<f64>,
    /// A blind hole's drill point: its full angle (radians), or a right angle's double (π) for a
    /// flat bottom.
    pub tip: Option<f64>,
    /// The thread whose tap drill the bore is.
    pub thread: Option<&'static str>,
    /// Every face it is made of: the bore, a counterbore's bore and step, the cones, the bottom.
    pub faces: BTreeSet<usize>,
}

impl Formed {
    /// Whether anything makes it more than a round hole Cut It would cut as well (a flat bottom
    /// alone does not: Cut It cuts one).
    pub fn is_formed(&self) -> bool {
        self.counterbore.is_some() || self.countersink.is_some() || self.tip.is_some_and(|t| t < std::f64::consts::PI) || self.thread.is_some()
    }
}

/// `value` (mm) as designed: the roundest of a whole, a half and a tenth of a millimetre within
/// `within`.
pub fn snapped(value: f64, within: f64) -> f64 {
    [1.0, 2.0, 10.0].into_iter().map(|per_mm| (value * per_mm).round() / per_mm).find(|v| (v - value).abs() <= within).unwrap_or(value)
}

fn snapped_angle(angle: f64) -> f64 {
    USUAL_ANGLES.iter().map(|d| d.to_radians()).find(|a| (a - angle).abs() <= SNAP_ANGLE).unwrap_or(angle)
}

fn unit(v: Vec3) -> Vec3 {
    v.normalized().unwrap_or(Vec3::ZERO)
}

/// Whether the line through `point` along `direction` is the axis through `on` along `axis`
/// (both unit), within `reach`.
fn coaxial(point: Vec3, direction: Vec3, on: Vec3, axis: Vec3, reach: f64) -> bool {
    let v = point - on;
    direction.dot(axis).abs() >= SAME_DIRECTION && (v - axis * v.dot(axis)).len() <= reach
}

/// The holes among `faces` made as a Hole feature makes them. `is_hole` tells a round hole Cut It
/// would drill; `span` how far a face runs along a direction (least, most); `middle` is the
/// scan's; `tolerance` the scan's (what sizes are snapped within). The faces taken by an earlier
/// hole (or listed in `taken`) are not read again. Only holes [`Formed::is_formed`] are returned.
pub fn read(
    faces: &[Face],
    is_hole: &dyn Fn(usize) -> bool,
    span: &dyn Fn(usize, Vec3) -> Option<(f64, f64)>,
    middle: Vec3,
    tolerance: f64,
    size: f64,
    taken: &BTreeSet<usize>,
) -> Vec<Formed> {
    let within = SNAP_SHARE * tolerance;
    let mut used: BTreeSet<usize> = taken.clone();
    // The narrowest bores first: a counterbore's wide bore is read with the bore it widens.
    let mut bores: Vec<(usize, f64)> = (0..faces.len())
        .filter(|&k| is_hole(k))
        .filter_map(|k| match faces[k].shape {
            Shape::Cylinder { radius, .. } if faces[k].area > 0.0 => Some((k, radius)),
            _ => None,
        })
        .collect();
    bores.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let mut out = Vec::new();
    for (bore, radius) in bores {
        if used.contains(&bore) {
            continue;
        }
        let Shape::Cylinder { point, axis, .. } = faces[bore].shape else { continue };
        let axis = unit(axis);
        let reach = (SAME_AXIS * radius).max(0.2 * tolerance).max(0.05);
        let Some((low, high)) = span(bore, axis) else { continue };
        let level = |p: Vec3| p.dot(axis);
        // What sits on the axis with it.
        let mut wider: Option<(usize, f64, (f64, f64))> = None;
        let mut cones: Vec<(usize, Vec3, f64, (f64, f64), f64)> = Vec::new();
        let mut flats: Vec<(usize, f64)> = Vec::new();
        // Every plane square to the axis, by its level: the face the hole opens in is one.
        let mut levels: Vec<f64> = Vec::new();
        for (j, face) in faces.iter().enumerate() {
            if j == bore || used.contains(&j) || face.area <= 0.0 {
                continue;
            }
            match face.shape {
                Shape::Cylinder { point: p, axis: a, radius: r } if is_hole(j) && r > radius + within && coaxial(p, unit(a), point, axis, reach) => {
                    if let Some(s) = span(j, axis)
                        && wider.as_ref().is_none_or(|w| r < w.1)
                    {
                        wider = Some((j, r, s));
                    }
                }
                Shape::Cone { apex, axis: a, half_angle } if coaxial(apex, unit(a), point, axis, reach.max(radius * 0.3)) => {
                    if let Some(s) = span(j, axis) {
                        cones.push((j, unit(a), half_angle, s, level(apex)));
                    }
                }
                Shape::Plane { point: p, normal } if unit(normal).dot(axis).abs() >= SAME_DIRECTION => {
                    levels.push(level(p));
                    let v = p - point;
                    if (v - axis * v.dot(axis)).len() <= reach.max(0.5 * radius) && face.area <= 1.5 * std::f64::consts::PI * radius * radius * 4.0 {
                        flats.push((j, level(p)));
                    }
                }
                _ => {}
            }
        }
        let near = |a: f64, b: f64| (a - b).abs() <= (0.1 * radius).max(2.0 * tolerance);
        // Which way the mouth is: the wider bore's side, else a cone's that opens there (a
        // countersink's axis grows toward the mouth, as a drill point's does), else a flat bottom's
        // other side, else the end further from the middle of the scan.
        let up = if let Some((_, _, (a, b))) = wider {
            if a + b > low + high { 1.0 } else { -1.0 }
        } else if let Some(c) = cones.first() {
            if c.1.dot(axis) > 0.0 { 1.0 } else { -1.0 }
        } else if let Some(&(_, at)) = flats.iter().find(|f| near(f.1, low) || near(f.1, high)) {
            if near(at, low) { 1.0 } else { -1.0 }
        } else {
            let (a, b) = (point + axis * (low - level(point)), point + axis * (high - level(point)));
            if b.dist(middle) >= a.dist(middle) { 1.0 } else { -1.0 }
        };
        // Along the grid's axis when it is that near it, as a drill is set square to a face: an
        // axis a few millionths off leaves the kernel's boolean slivers to cut.
        let out_dir = crate::reconstruct::on_grid(axis * up);
        // Levels measured outward: the bore runs from `inner` to `outer`.
        let (inner, outer) = if up > 0.0 { (low, high) } else { (-high, -low) };
        let out_of = |s: (f64, f64)| if up > 0.0 { s } else { (-s.1, -s.0) };
        let mut faces_of: BTreeSet<usize> = BTreeSet::from([bore]);
        let mut mouth_at = outer;
        let mut counterbore = None;
        let mut step_at = None;
        if let Some((j, r, s)) = wider {
            let (a, b) = out_of(s);
            // The wider bore starts where this one ends, and runs to the mouth.
            if near(a, outer) && b > a {
                mouth_at = b;
                counterbore = Some((r, b - a));
                step_at = Some(a);
                faces_of.insert(j);
                // Its step: a flat face at the level the two meet.
                for &(f, at) in &flats {
                    if near(at * up, a) {
                        faces_of.insert(f);
                    }
                }
            }
        }
        let mut countersink = None;
        let mut tip = None;
        // The mouth at the face it opens in, where there is one that near: the scan's ends of a
        // bore or a cone are a little ragged.
        let face_at = |t: f64| levels.iter().map(|l| l * up).filter(|l| near(*l, t)).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs()));
        let mut sunk = None;
        for &(j, direction, half, s, apex) in &cones {
            let (a, b) = out_of(s);
            let opens_out = direction.dot(out_dir) > 0.0;
            if !opens_out {
                continue;
            }
            if counterbore.is_none() && countersink.is_none() && near(a, outer) && b > a {
                // At the mouth, growing toward it: its radius there, from its apex.
                mouth_at = mouth_at.max(b);
                sunk = Some((apex * up, half));
                countersink = Some((radius + (b - a) * half.tan(), 2.0 * half));
                faces_of.insert(j);
            } else if tip.is_none() && near(b, inner) && b > a {
                let full = 2.0 * half;
                if full >= TIP_ANGLES.0.to_radians() && full <= TIP_ANGLES.1.to_radians() {
                    tip = Some(full);
                    faces_of.insert(j);
                }
            }
        }
        if let Some(t) = face_at(mouth_at) {
            mouth_at = t;
        }
        // A counterbore as deep as its step, a face square to the axis where there is one (two
        // steps in one plane are one surface, whose middle lies on neither axis).
        if let (Some((r, _)), Some(a)) = (counterbore, step_at) {
            counterbore = Some((r, mouth_at - face_at(a).unwrap_or(a)));
        }
        if let Some((apex, half)) = sunk {
            countersink = Some(((mouth_at - apex) * half.tan(), 2.0 * half));
        }
        let inner = if tip.is_none() { face_at(inner).unwrap_or(inner) } else { inner };
        if tip.is_none()
            && let Some(&(f, _)) = flats.iter().find(|f| near(f.1 * up, inner))
        {
            tip = Some(std::f64::consts::PI);
            faces_of.insert(f);
        }
        let at = |t: f64| point + out_dir * (t - level(point) * up);
        let diameter = snapped(2.0 * radius, within);
        let thread = if counterbore.is_none() && countersink.is_none() {
            TAP_DRILLS.iter().find(|(_, d)| (2.0 * radius - d).abs() <= (TAP_SHARE * d).min(within.max(0.01))).map(|(name, _)| *name)
        } else {
            None
        };
        let formed = Formed {
            bore,
            radius: 0.5 * diameter,
            mouth: at(mouth_at),
            inward: -out_dir,
            bottom: at(inner),
            counterbore: counterbore.map(|(r, d)| (0.5 * snapped(2.0 * r, within), snapped(d, within))),
            countersink: countersink.map(|(r, a)| (0.5 * snapped(2.0 * r, 2.0 * within), snapped_angle(a))),
            depth: tip.map(|_| snapped(mouth_at - inner, within)),
            tip: tip.map(|a| if a >= std::f64::consts::PI { a } else { snapped_angle(a) }),
            thread,
            faces: faces_of,
        };
        let drilled = formed.radius <= MOST_BORE_SHARE * size
            && formed.counterbore.is_none_or(|(r, _)| r <= MOST_HEAD * formed.radius)
            && formed.countersink.is_none_or(|(r, _)| r <= MOST_HEAD * formed.radius)
            && (formed.tip != Some(std::f64::consts::PI) || formed.depth.is_some_and(|d| d >= LEAST_FLAT_DEPTH * 2.0 * formed.radius));
        if drilled && formed.is_formed() {
            used.extend(formed.faces.iter().copied());
            out.push(formed);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cylinder(at: Vec3, radius: f64) -> Face {
        Face { shape: Shape::Cylinder { point: at, axis: Vec3::Z, radius }, area: 100.0 }
    }

    /// The faces' spans along Z, given per face (least, most).
    fn reading(faces: &[Face], spans: &[(f64, f64)], middle: Vec3) -> Vec<Formed> {
        let holes: BTreeSet<usize> = (0..faces.len()).filter(|&k| matches!(faces[k].shape, Shape::Cylinder { .. })).collect();
        read(
            faces,
            &|k| holes.contains(&k),
            &|k, d: Vec3| spans.get(k).map(|&(a, b)| if d.z >= 0.0 { (a * d.z, b * d.z) } else { (b * d.z, a * d.z) }),
            middle,
            0.12,
            120.0,
            &BTreeSet::new(),
        )
    }

    #[test]
    fn a_counterbore_is_one_hole_opening_at_its_wide_end() {
        // A plate from z 0 to 12: a 6.6 bore to 5.5, an 11 counterbore from there to the top.
        let faces = [
            cylinder(Vec3::new(-35.0, 15.0, 2.75), 3.292),
            cylinder(Vec3::new(-35.0, 15.0, 8.75), 5.495),
            Face { shape: Shape::Plane { point: Vec3::new(-35.0, 15.0, 5.5), normal: Vec3::Z }, area: 61.0 },
        ];
        let found = reading(&faces, &[(0.0, 5.5), (5.5, 12.0), (5.5, 5.5)], Vec3::new(0.0, 0.0, 6.0));
        assert_eq!(found.len(), 1, "{found:?}");
        let h = &found[0];
        assert!((h.radius - 3.3).abs() < 1e-9 && h.mouth.dist(Vec3::new(-35.0, 15.0, 12.0)) < 1e-9 && h.inward.z < -0.99, "{h:?}");
        assert_eq!(h.counterbore, Some((5.5, 6.5)), "{h:?}");
        assert_eq!((h.depth, h.tip, h.thread), (None, None, None), "through: {h:?}");
        assert_eq!(h.faces, BTreeSet::from([0, 1, 2]));
    }

    #[test]
    fn a_countersink_a_drill_point_and_a_tap_drill_are_read() {
        let cone = |apex: Vec3, half: f64| Face { shape: Shape::Cone { apex, axis: Vec3::Z, half_angle: half }, area: 80.0 };
        let faces = [
            // A 5.5 bore through, a 90° countersink 10.4 across at the top.
            cylinder(Vec3::new(0.0, 15.0, 4.8), 2.745),
            cone(Vec3::new(0.0, 15.0, 6.8), 45f64.to_radians()),
            // An 8 bore 8 deep from the top, its 118° point below.
            cylinder(Vec3::new(35.0, -15.0, 8.0), 3.99),
            cone(Vec3::new(35.0, -15.0, 1.6), 58.9f64.to_radians()),
            // A 5.0 bore through: tapped M6.
            cylinder(Vec3::new(35.0, 15.0, 6.0), 2.496),
            // A 7 bore through: a plain hole, left to Cut It.
            cylinder(Vec3::new(-10.0, 0.0, 6.0), 3.5),
        ];
        let spans = [(0.0, 9.55), (9.55, 12.0), (4.0, 12.0), (1.6, 4.0), (0.0, 12.0), (0.0, 12.0)];
        let found = reading(&faces, &spans, Vec3::new(0.0, 0.0, 6.0));
        assert_eq!(found.len(), 3, "{found:?}");
        let sunk = found.iter().find(|h| h.bore == 0).unwrap();
        let (r, a) = sunk.countersink.unwrap();
        assert!((r - 5.2).abs() < 1e-9 && (a - 90f64.to_radians()).abs() < 1e-9 && sunk.radius == 2.75, "{sunk:?}");
        assert!(sunk.mouth.z == 12.0 && sunk.depth.is_none(), "{sunk:?}");
        let blind = found.iter().find(|h| h.bore == 2).unwrap();
        assert_eq!(blind.depth, Some(8.0), "{blind:?}");
        assert!((blind.tip.unwrap() - 118f64.to_radians()).abs() < 1e-9 && blind.radius == 4.0 && blind.mouth.z == 12.0, "{blind:?}");
        let tapped = found.iter().find(|h| h.bore == 4).unwrap();
        assert_eq!(tapped.thread, Some("M6"), "{tapped:?}");
    }

    #[test]
    fn sizes_are_read_as_designed_within_the_tolerance() {
        assert_eq!(snapped(6.584, 0.03), 6.6);
        assert_eq!(snapped(10.99, 0.03), 11.0);
        assert_eq!(snapped(6.47, 0.03), 6.47);
        assert_eq!(snapped(5.98, 0.03), 6.0);
    }
}
