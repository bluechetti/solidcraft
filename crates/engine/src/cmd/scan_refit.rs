//! Refit to Scan (`scan.refit`): a Scan Surface made by New Free Form or Fit Patches keeps a recipe
//! (`solidcraft_doc::scan_recipe`): the scan, the command and its parameters, and where the fit put
//! each control point. Refit runs the command again on the scan as it is now and lays on the new
//! fit what was done by hand since (the surface's control points against that baseline), so a
//! re-scan, or a change earlier in the timeline that moved or cleaned the scan, costs a click and
//! not the hand work.

use serde_json::{Value, json};
use solidcore_record::Vec3;
use solidcraft_doc::scan_recipe::ScanRecipe;
use solidcraft_geom::prelude::*;
use solidcraft_scan::surface_edit::{points, with_points};

use super::CommandSpec;
use super::surface::{EditableSurface, editable, network_step, surface_step, unmoved, write_step};
use crate::params::{bad, bool_, str_};
use crate::schema::scan_refit::{ScanRefitParams, ScanRefitResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{EngineError, Result, Session};

const CONTRACT: TypedContract<ScanRefitParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("scan.refit", "Refit to Scan", crate::checked!("scan.refit", refit))
        .at("SCAN", "FREEFORM")
        .icon("scan_refit")
        .contract(&CONTRACT)
        .output(output_schema_for::<ScanRefitResult>)
        .params("surface?: the surface body (default: the selected one, else the one made last); discard_edits?: bool (default false: fit afresh and drop the hand edits; needed when a face or a row of the net was added or taken away by hand): the command that made the surface (New Free Form, Fit Patches) runs again with its parameters on the scan as it is now, and the control points moved by hand are moved the same on the new fit; one undo step → surface, scan, faces, carried, discarded, feature"),
];

/// The recipe for `faces` just fitted to `scan` by `command` with `params`: the baseline is where
/// the fit put each control point.
pub fn recipe(scan: &str, command: &str, params: Value, faces: &[solidcraft_geom::bspline::NurbsSurface]) -> ScanRecipe {
    let baseline = faces.iter().map(|f| points(f).iter().map(|p| [p.x, p.y, p.z]).collect()).collect();
    ScanRecipe::new(scan, command, params, baseline)
}

/// The recipe of the Scan Surface feature `id`, if it has one.
pub fn recipe_of(s: &Session, id: u64) -> Option<ScanRecipe> {
    match s.doc.features.iter().find(|f| f.id == id).map(|f| &f.kind) {
        Some(solidcraft_doc::FeatureKind::ScanSurface { recipe, .. }) => recipe.clone(),
        _ => None,
    }
}

/// The faces the recipe's command makes now.
fn fresh(s: &Session, r: &ScanRecipe, cmd: &str) -> Result<Vec<solidcraft_geom::bspline::NurbsSurface>> {
    match r.command.as_str() {
        "scan.free_form" => Ok(vec![super::free_form::made_from_params(s, &r.params)?.surface]),
        "scan.fit_patches" => {
            // A region kept as triangle numbers means the same triangles only while the scan has the
            // same number of them: cleaning or splitting it numbers them anew.
            if let Some(then) = r.params.get("of_triangles").and_then(Value::as_u64) {
                let now = super::mesh(s, &json!({ "body": r.scan }), cmd)?.triangles.len() as u64;
                if now != then {
                    return Err(bad(
                        cmd,
                        format!(
                            "`{}` has {now} triangles now and had {then} when the surface was fitted to the part selected on it, so the part is not the same: select it again and fit again",
                            r.scan
                        ),
                    ));
                }
            }
            Ok(super::patches::fitted(s, &r.params, cmd)?.3.surfaces)
        }
        other => Err(bad(cmd, format!("the recipe names `{other}`, which cannot be run again"))),
    }
}

fn middle(points: &[Vec3]) -> Vec3 {
    points.iter().fold(Vec3::ZERO, |s, p| s + *p) / points.len().max(1) as f64
}

/// For each face of the baseline, the face of `current` that is it: one face is the one, several
/// are matched by where their middles are (hand edits are small beside the faces).
fn match_faces(baseline: &[Vec<Vec3>], current: &[Vec<Vec3>]) -> Option<Vec<usize>> {
    if baseline.len() != current.len() {
        return None;
    }
    let mut taken = vec![false; current.len()];
    let mut out = Vec::with_capacity(baseline.len());
    for b in baseline {
        let m = middle(b);
        let (best, _) = current
            .iter()
            .enumerate()
            .filter(|(j, c)| !taken[*j] && c.len() == b.len())
            .map(|(j, c)| (j, middle(c).dist(m)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;
        taken[best] = true;
        out.push(best);
    }
    Some(out)
}

/// The new fit's faces with what was done by hand laid on them: each control point moved from the
/// baseline in `current` is moved the same from where the new fit puts it. Also how many were.
fn laid(
    fit: &[solidcraft_geom::bspline::NurbsSurface],
    recipe: &ScanRecipe,
    current: &[solidcraft_geom::bspline::NurbsSurface],
    cmd: &str,
) -> Result<(Vec<solidcraft_geom::bspline::NurbsSurface>, usize)> {
    let reshaped = || {
        bad(
            cmd,
            "the surface was reshaped by hand (a face or a row of the net added or taken away), so the fit cannot carry its edits over: give `discard_edits: true` to fit it afresh",
        )
    };
    let baseline: Vec<Vec<Vec3>> = recipe.baseline.iter().map(|f| f.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect()).collect();
    let now: Vec<Vec<Vec3>> = current.iter().map(points).collect();
    let fresh: Vec<Vec<Vec3>> = fit.iter().map(points).collect();
    if fresh.len() != baseline.len() || fresh.iter().zip(&baseline).any(|(a, b)| a.len() != b.len()) {
        return Err(bad(
            cmd,
            "the fit now has another number of faces or control points than when the surface was made: give `discard_edits: true` to fit it afresh",
        ));
    }
    let of = match_faces(&baseline, &now).ok_or_else(reshaped)?;
    let mut carried = 0;
    let mut faces = Vec::with_capacity(fit.len());
    for (i, face) in fit.iter().enumerate() {
        let mut net = fresh[i].clone();
        for (k, p) in net.iter_mut().enumerate() {
            let moved = now[of[i]][k] - baseline[i][k];
            if moved.len() > 1e-9 {
                *p += moved;
                carried += 1;
            }
        }
        faces.push(with_points(face, &net).ok_or_else(|| bad(cmd, "the refitted face could not take the edits"))?);
    }
    Ok((faces, carried))
}

/// `faces` (in the surface's own coordinates) as the surface is placed now, for what is kept of a
/// Free Form besides its faces.
fn placed(
    target: &EditableSurface,
    faces: &[solidcraft_geom::bspline::NurbsSurface],
    cmd: &str,
) -> Result<Vec<solidcraft_geom::bspline::NurbsSurface>> {
    let Some(by) = &target.placed else { return Ok(faces.to_vec()) };
    faces
        .iter()
        .map(|f| {
            let moved: Vec<Vec3> = points(f).iter().map(|p| by.apply(*p)).collect();
            with_points(f, &moved).ok_or_else(|| bad(cmd, "the surface's place could not be applied"))
        })
        .collect()
}

fn refit(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "scan.refit";
    let target = editable(s, str_(p, "surface"), cmd)?;
    let recipe = recipe_of(s, target.feature).ok_or_else(|| {
        bad(
            cmd,
            format!(
                "`{}` has no recipe: only a surface made by New Free Form or Fit Patches (from a click, or a part selected, on a scan) can be fitted again",
                target.body
            ),
        )
    })?;
    let discard = bool_(p, "discard_edits") == Some(true);
    let fit = fresh(s, &recipe, cmd)?;
    let current: Vec<_> = target.faces.iter().map(|f| unmoved(&target, f, cmd)).collect::<Result<_>>()?;
    let (faces, carried) = if discard { (fit.clone(), 0) } else { laid(&fit, &recipe, &current, cmd)? };
    let step = match faces.as_slice() {
        [one] => surface_step(one, cmd)?,
        many => network_step(many, cmd)?,
    };
    write_step(s, &target, step, cmd)?;
    // The new fit is the baseline from now on.
    let renewed = super::refit::recipe(&recipe.scan, &recipe.command, recipe.params.clone(), &fit);
    if let Some(solidcraft_doc::FeatureKind::ScanSurface { recipe: kept, .. }) = s.doc_mut().feature_mut(target.feature).map(|f| &mut f.kind) {
        *kept = Some(renewed);
    }
    s.refresh();
    if faces.len() > 1 {
        let now = placed(&target, &faces, cmd)?;
        let joins = super::network_edit::joins_for(s, &target, &now, cmd)?;
        super::network_edit::keep(s, &target, &now, &joins, None);
    }
    let reply = ScanRefitResult {
        surface: target.body,
        scan: recipe.scan,
        faces: faces.len(),
        carried,
        discarded: discard,
        feature: target.feature,
        param_warnings: Vec::new(),
    };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use solidcore_record::Vec3;

    use crate::Session;

    fn run(s: &mut Session, id: &str, p: Value) -> Value {
        s.execute(id, &p).unwrap_or_else(|e| panic!("{id}: {e}"))
    }
    use serde_json::Value;

    /// A scan of a dome, `z = height − 0.004 r²` over 60 × 60, `n` squares a side, as an STL.
    fn dome_stl(n: u32, height: f64) -> String {
        let at = |i: u32, j: u32| {
            let (x, y) = (60.0 * f64::from(i) / f64::from(n), 60.0 * f64::from(j) / f64::from(n));
            Vec3::new(x, y, height - 0.004 * ((x - 30.0).powi(2) + (y - 30.0).powi(2)))
        };
        let mut stl = String::from("solid d\n");
        for i in 0..n {
            for j in 0..n {
                for t in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                    stl += "facet normal 0 0 0\nouter loop\n";
                    for p in t {
                        stl += &format!("vertex {} {} {}\n", p.x, p.y, p.z);
                    }
                    stl += "endloop\nendfacet\n";
                }
            }
        }
        stl + "endsolid d\n"
    }

    fn centre_z(s: &Session, body: &str) -> f64 {
        let t = super::editable(s, Some(body), "test").unwrap();
        t.surface.eval(0.5, 0.5).unwrap().z
    }

    /// A Free Form fitted to a dome keeps its recipe; a control point dragged by hand; the scan
    /// replaced by a dome 3 mm higher; Refit fits it again on the new scan and the hand move is
    /// laid on the new fit, one undo step; a surface reshaped by hand refuses until the edits are
    /// discarded; a surface with no recipe says so.
    #[test]
    fn refit_follows_a_new_scan_and_keeps_the_hand_edits() {
        let dir = std::env::temp_dir().join(format!("solidcraft-scan-refit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let low = dir.join("low.stl");
        std::fs::write(&low, dome_stl(40, 8.0)).unwrap();
        let mut s = Session::new(solidcraft_doc::Document::new("t"));
        run(&mut s, "file.insert_mesh", json!({"path": low.to_string_lossy()}));
        let made = run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "count": 5}));
        let surface = made["surface"].as_str().unwrap().to_owned();
        let recipe = super::recipe_of(&s, made["feature"]["feature"].as_u64().unwrap()).expect("a recipe kept with the surface");
        assert_eq!((recipe.command.as_str(), recipe.baseline.len(), recipe.baseline[0].len()), ("scan.free_form", 1, 25));
        let before = centre_z(&s, &surface);

        // A control point dragged up by hand, 2 mm.
        let now = super::editable(&s, Some(&surface), "test").unwrap().surface;
        let up = solidcraft_scan::surface_edit::points(&now)[12] + Vec3::new(0.0, 0.0, 2.0);
        run(&mut s, "scan.edit_surface", json!({"moves": [{"point": [2, 2], "to": [up.x, up.y, up.z]}]}));
        let edited = centre_z(&s, &surface);
        assert!(edited > before + 0.3, "the hand edit raised the centre: {before} -> {edited}");

        // The scan again, 3 mm higher: the old scan goes and the new one is where it was.
        // The same file read again, now 3 mm higher: the scan keeps its name.
        let scan = recipe.scan.clone();
        let scan_feature = s.model.state().body(&scan).map(|b| b.feature).unwrap();
        run(&mut s, "timeline.delete", json!({"feature": scan_feature.to_string()}));
        std::fs::write(&low, dome_stl(40, 11.0)).unwrap();
        run(&mut s, "file.insert_mesh", json!({"path": low.to_string_lossy()}));
        let reply = run(&mut s, "scan.refit", json!({"surface": surface}));
        assert_eq!((reply["carried"].as_u64(), reply["discarded"].as_bool(), reply["faces"].as_u64()), (Some(1), Some(false), Some(1)), "{reply}");
        let after = centre_z(&s, &surface);
        assert!((after - (edited + 3.0)).abs() < 0.6, "up 3 mm with the scan and the hand move kept: {edited} -> {after}");

        // Discarding the edits fits it afresh: on the new dome, without the 2 mm.
        let afresh = run(&mut s, "scan.refit", json!({"surface": surface, "discard_edits": true}));
        assert_eq!(afresh["carried"].as_u64(), Some(0), "{afresh}");
        assert!((centre_z(&s, &surface) - (before + 3.0)).abs() < 0.6);

        // One undo step brings back the step before.
        run(&mut s, "edit.undo", json!({}));
        assert!((centre_z(&s, &surface) - after).abs() < 1e-6, "one undo step");

        // A surface reshaped by hand cannot carry its edits over.
        run(&mut s, "scan.edit_net", json!({"op": "refine"}));
        let refused = s.execute("scan.refit", &json!({"surface": surface})).unwrap_err().to_string();
        assert!(refused.contains("discard_edits"), "{refused}");
        // And one with no recipe says so.
        let plain = run(&mut s, "scan.free_form", json!({"corners": [[10, 10, 0], [50, 10, 0], [50, 50, 0], [10, 50, 0]], "plane": "XY"}));
        let none = s.execute("scan.refit", &json!({"surface": plain["surface"]})).unwrap_err().to_string();
        assert!(none.contains("no recipe"), "{none}");
    }
}
