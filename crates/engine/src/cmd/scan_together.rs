//! Fit Together (`scan.fit_together`): shapes fitted to their regions of a scan and to relations
//! between them at once (`solidcraft_scan::relate`): two bores on one axis, a face square to Z,
//! two floors 25 mm apart. Each shape's region is chosen as Fit Plane and the others choose theirs
//! (a click grown, round the click, a kept selection, triangles); the relations are given, or with
//! none given the ones the shapes nearly hold already are taken. Each shape held is kept as the
//! construction plane, axis or point its Fit tool keeps, one undo step.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_geom::Vec3Geom as _;
use solidcraft_scan::fit::{self, Shape};
use solidcraft_scan::relate::{self, Datum, Held, Kind as Rel, Relation, Side};

use crate::params::bad;
use crate::schema::scan::{
    ScanFittedCone, ScanFittedCylinder, ScanFittedPlane, ScanFittedSphere, ScanFittedTorus, ScanTogetherRelation, ScanTogetherResult,
    ScanTogetherShape,
};
use crate::{EngineError, Result, Session};

/// The most shapes fitted together.
pub const MOST_SHAPES: usize = 20;
/// The most relations held.
pub const MOST_RELATIONS: usize = 100;

fn kind_of(name: &str) -> Option<fit::Kind> {
    Some(match name {
        "plane" => fit::Kind::Plane,
        "cylinder" => fit::Kind::Cylinder,
        "sphere" => fit::Kind::Sphere,
        "cone" => fit::Kind::Cone,
        "torus" => fit::Kind::Torus,
        _ => return None,
    })
}

fn kind_name(s: &Shape) -> &'static str {
    match s {
        Shape::Plane { .. } => "plane",
        Shape::Cylinder { .. } => "cylinder",
        Shape::Sphere { .. } => "sphere",
        Shape::Cone { .. } => "cone",
        Shape::Torus { .. } => "torus",
    }
}

fn datum(name: &str) -> Option<Datum> {
    Some(match name.to_ascii_lowercase().as_str() {
        "x" => Datum::X,
        "y" => Datum::Y,
        "z" => Datum::Z,
        "xy" => Datum::PlaneXy,
        "yz" => Datum::PlaneYz,
        "zx" | "xz" => Datum::PlaneZx,
        "origin" => Datum::Origin,
        _ => return None,
    })
}

fn datum_name(d: Datum) -> &'static str {
    match d {
        Datum::X => "x",
        Datum::Y => "y",
        Datum::Z => "z",
        Datum::PlaneXy => "xy",
        Datum::PlaneYz => "yz",
        Datum::PlaneZx => "zx",
        Datum::Origin => "origin",
    }
}

/// A relation as the command takes it: `{kind, a, b?, value?}`.
fn relation(s: &Session, v: &Value, count: usize, cmd: &str) -> Result<Relation> {
    let kind = v.get("kind").and_then(Value::as_str).ok_or_else(|| bad(cmd, "a relation needs `kind`"))?;
    let index = |key: &str| -> Result<Option<usize>> {
        match v.get(key).filter(|x| !x.is_null()) {
            None => Ok(None),
            Some(x) => {
                let i = x.as_u64().map(|i| i as usize).ok_or_else(|| bad(cmd, format!("`{key}` is a shape's number from 0")))?;
                if i >= count {
                    return Err(bad(cmd, format!("`{key}` {i}: there are {count} shapes (numbered from 0)")));
                }
                Ok(Some(i))
            }
        }
    };
    let a = index("a")?.ok_or_else(|| bad(cmd, "a relation needs `a`, a shape's number from 0"))?;
    let b = match v.get("b").filter(|x| !x.is_null()) {
        None => None,
        Some(Value::String(name)) => {
            Some(Side::Datum(datum(name).ok_or_else(|| bad(cmd, format!("`b` `{name}`: a shape's number, or x, y, z, xy, yz, zx or origin")))?))
        }
        Some(_) => index("b")?.map(Side::Shape),
    };
    let value = |what: solidcraft_doc::expr::Kind| -> Result<f64> {
        let e = crate::params::expr(v, "value").ok_or_else(|| bad(cmd, format!("a `{kind}` relation needs `value`")))?;
        s.doc.eval(&e, what).map_err(|e| bad(cmd, format!("value: {e}")))
    };
    let kind = match kind {
        "parallel" => Rel::Parallel,
        "perpendicular" => Rel::Perpendicular,
        "coaxial" => Rel::Coaxial,
        "equal_radius" => Rel::EqualRadius,
        "distance" => Rel::Distance(value(solidcraft_doc::expr::Kind::Length)?),
        "radius" => Rel::Radius(value(solidcraft_doc::expr::Kind::Length)?),
        "angle" => Rel::Angle(value(solidcraft_doc::expr::Kind::Angle)?),
        other => {
            return Err(bad(cmd, format!("`kind` `{other}`: parallel, perpendicular, coaxial, equal_radius, distance, angle or radius")));
        }
    };
    Ok(Relation { kind, a, b })
}

/// The shape's reply pieces: a plane, a cylinder, a sphere, a cone or a torus.
fn shape_reply(shape: &Shape, middle: Vec3, half_length: f64) -> ScanTogetherShape {
    let at = |v: Vec3| [v.x, v.y, v.z];
    let mut out = ScanTogetherShape {
        kind: kind_name(shape).to_owned(),
        plane: None,
        cylinder: None,
        sphere: None,
        cone: None,
        torus: None,
        rms: 0.0,
        max: 0.0,
        free_rms: 0.0,
        free_max: 0.0,
        feature: None,
    };
    match *shape {
        Shape::Plane { normal, .. } => out.plane = Some(ScanFittedPlane { point: at(middle), normal: at(normal) }),
        Shape::Cylinder { axis, radius, .. } => {
            out.cylinder = Some(ScanFittedCylinder { point: at(middle), axis: at(axis), radius, diameter: 2.0 * radius, length: 2.0 * half_length })
        }
        Shape::Sphere { center, radius } => out.sphere = Some(ScanFittedSphere { center: at(center), radius, diameter: 2.0 * radius }),
        Shape::Cone { apex, axis, half_angle } => {
            out.cone = Some(ScanFittedCone { apex: at(apex), axis: at(axis), half_angle: half_angle.to_degrees() })
        }
        Shape::Torus { center, axis, major, minor } => out.torus = Some(ScanFittedTorus { center: at(center), axis: at(axis), major, minor }),
    }
    out
}

/// The middle of a free fit's region moved onto the held shape: onto a plane, onto a round
/// shape's axis (where its free fit had it).
fn onto(shape: &Shape, middle: Vec3) -> Vec3 {
    match *shape {
        Shape::Plane { .. } => middle - shape.normal_at(middle).unwrap_or(Vec3::ZERO) * shape.distance(middle),
        Shape::Sphere { center, .. } | Shape::Torus { center, .. } => center,
        Shape::Cylinder { point, axis, .. } | Shape::Cone { apex: point, axis, .. } => point + axis * (middle - point).dot(axis),
    }
}

pub(super) fn fit_together(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.fit_together";
    let given = p.get("shapes").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`shapes` lists the shapes to fit, [{kind, point, …}, …]"))?;
    if given.len() < 2 || given.len() > MOST_SHAPES {
        return Err(bad(cmd, format!("fit 2 to {MOST_SHAPES} shapes together")));
    }
    let tolerance = super::fit_tolerance(s, p, cmd)?;
    let keep = crate::params::bool_(p, "keep").unwrap_or(true);
    let body = crate::params::str_(p, "body").map(str::to_owned);
    // Each shape fitted alone first, on its own region.
    let mut free: Vec<(super::Mesh, fit::Fitted)> = Vec::with_capacity(given.len());
    for (i, shape) in given.iter().enumerate() {
        let kind = shape
            .get("kind")
            .and_then(Value::as_str)
            .and_then(kind_of)
            .ok_or_else(|| bad(cmd, format!("shape {i}: `kind` is plane, cylinder, sphere, cone or torus")))?;
        let mut q = shape.clone();
        if let (Some(o), Some(b)) = (q.as_object_mut(), body.as_ref())
            && !o.contains_key("body")
        {
            o.insert("body".into(), json!(b));
        }
        let fitted = super::fitted_shape(s, &q, cmd, kind, tolerance).map_err(|e| bad(cmd, format!("shape {i}: {e}")))?;
        free.push(fitted);
    }
    if free.windows(2).any(|w| w[0].0.body != w[1].0.body) {
        return Err(bad(cmd, "the shapes must all be on one scan"));
    }
    let shapes: Vec<Shape> = free.iter().map(|(_, f)| f.shape).collect();
    let (relations, suggested) = match p.get("relations").filter(|v| !v.is_null()) {
        Some(Value::Array(list)) => {
            if list.len() > MOST_RELATIONS {
                return Err(bad(cmd, format!("at most {MOST_RELATIONS} relations")));
            }
            (list.iter().map(|r| relation(s, r, shapes.len(), cmd)).collect::<Result<Vec<_>>>()?, false)
        }
        Some(_) => return Err(bad(cmd, "`relations` lists {kind, a, b?, value?}")),
        None => (relate::suggest(&shapes), true),
    };
    let held: Vec<Held> = free
        .iter()
        .map(|(mesh, f)| {
            let scan = mesh.scan();
            let mut points: Vec<Vec3> = f.triangles.iter().filter_map(|&t| scan.corners(t)).flatten().collect();
            points.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z)));
            points.dedup();
            Held { shape: f.shape, points }
        })
        .collect();
    let solved = relate::solve(&held, &relations).map_err(|e| bad(cmd, e.to_string()))?;
    let mut replies = Vec::with_capacity(free.len());
    for (k, ((mesh, f), shape)) in free.iter().zip(&solved.shapes).enumerate() {
        let middle = onto(shape, f.middle);
        let mut reply = shape_reply(shape, middle, f.half_length);
        reply.rms = solved.rms[k];
        reply.max = solved.max[k];
        reply.free_rms = f.rms;
        reply.free_max = f.max;
        if keep {
            let kept = fit::Fitted { shape: *shape, middle, ..f.clone() };
            reply.feature = Some(super::keep_shape(s, &kept, mesh.scan().extent())?);
        }
        replies.push(reply);
    }
    let relations = relations
        .iter()
        .zip(&solved.off)
        .map(|(r, off)| ScanTogetherRelation {
            kind: r.kind.name().to_owned(),
            a: r.a,
            b: r.b.map(|b| match b {
                Side::Shape(i) => json!(i),
                Side::Datum(d) => json!(datum_name(d)),
            }),
            value: match r.kind {
                Rel::Distance(v) | Rel::Radius(v) => Some(v),
                Rel::Angle(v) => Some(v.to_degrees()),
                _ => None,
            },
            off: *off,
        })
        .collect();
    let reply = ScanTogetherResult {
        body: free.first().map(|(m, _)| m.body.clone()).unwrap_or_default(),
        shapes: replies,
        relations,
        suggested,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}
