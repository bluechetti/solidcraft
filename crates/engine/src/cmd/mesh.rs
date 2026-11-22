//! Parametric reconstruction of imported mechanical mesh bodies, and a mesh import read again
//! in its file's own length unit.

use serde_json::Value;
use solidcraft_doc::FeatureKind;
use solidcraft_doc::expr::Kind;

use super::{
    CommandSpec,
    features::{add_feature, check_expr},
};
use crate::schema::features::FeatureResult;
use crate::schema::mesh::{MeshConvertParams, MeshUnitsParams, MeshUnitsResult};
use crate::schema::{TypedContract, output_schema_for};
use crate::{
    EngineError, Result, Sel, Session,
    params::{bad, expr, str_},
};

const MESH_CONVERT_CONTRACT: TypedContract<MeshConvertParams> = TypedContract::new();
const UNITS_CONTRACT: TypedContract<MeshUnitsParams> = TypedContract::new();

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("mesh.convert", "Convert Mesh", crate::checked!("mesh.convert", convert)).begins_edit()
        .icon("import")
        .contract(&MESH_CONVERT_CONTRACT)
        .output(output_schema_for::<FeatureResult>)
        .params("body?: mesh body name (default: the selected mesh body); tolerance?: length expression (default 0.05 mm): rebuild a closed mesh as an editable solid — a constant-section part as planar and cylindrical faces, any other closed mesh as the faceted solid its triangles describe — with a parametric timeline feature in the mesh body's owning component (the active component must be that component or an ancestor; otherwise activate the named component first); the active component is unchanged; name?"),
    CommandSpec::new("mesh.units", "Mesh Units", crate::checked!("mesh.units", units))
        .icon("import")
        .contract(&UNITS_CONTRACT)
        .output(output_schema_for::<MeshUnitsResult>)
        .params("units: mm|cm|m|in|ft (the length unit the mesh file was saved in); body?: a body of the mesh import (default: the selected mesh body, else the last mesh import); from?: mm|cm|m|in|ft (the unit it was read in, default mm: Insert Mesh reads a file without `units` as millimetres): the import's points are scaled from `from` to `units` where they are, so a scan saved in metres comes in at its size, in one undo step; refused once a feature follows the import in the timeline (insert the file again with `units` instead) → feature, bodies, scale, units, from"),
];

/// `mesh.units`: a mesh import read in the wrong length unit, scaled to its file's own.
fn units(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mesh.units";
    let (to, k_to) = super::point_cloud::units_of(p, cmd)?.ok_or_else(|| bad(cmd, "`units` is required: mm, cm, m, in or ft"))?;
    let (from, k_from) = super::point_cloud::unit_of(p, "from", cmd)?.unwrap_or(("mm", 1.0));
    let state = s.model.state();
    let named = str_(p, "body").filter(|b| !b.trim().is_empty()).map(str::to_owned);
    let body = named.or_else(|| s.selection.iter().find_map(|x| if let Sel::Body { name } = x { Some(name.clone()) } else { None }));
    let feature = match &body {
        Some(b) => state.body(b).ok_or_else(|| bad(cmd, format!("no body `{b}`")))?.feature,
        None => s
            .doc
            .features
            .iter()
            .rev()
            .find(|f| matches!(f.kind, FeatureKind::MeshImport { .. }))
            .map(|f| f.id)
            .ok_or_else(|| bad(cmd, "the design has no mesh brought in by Insert Mesh"))?,
    };
    let what = body.as_deref().map_or_else(|| "the mesh".to_owned(), |b| format!("`{b}`"));
    if !s.doc.feature(feature).is_some_and(|f| matches!(f.kind, FeatureKind::MeshImport { .. })) {
        return Err(bad(cmd, format!("{what} is not a mesh brought in by Insert Mesh, or a later feature has changed it")));
    }
    // A later feature measured or placed against the mesh as it was: scaled under it, it would no
    // longer fit what it was built on.
    if s.doc.features.last().map(|f| f.id) != Some(feature) {
        return Err(bad(cmd, format!("a feature follows {what} in the timeline: insert the file again with its `units` instead")));
    }
    let scale = k_to / k_from;
    let bodies: Vec<String> = state.bodies.iter().filter(|b| b.feature == feature).map(|b| b.name.clone()).collect();
    if let Some(FeatureKind::MeshImport { meshes, .. }) = s.doc_mut().feature_mut(feature).map(|f| &mut f.kind) {
        solidcraft_io::scale_meshes(meshes, scale).map_err(|e| bad(cmd, e.to_string()))?;
    }
    s.refresh();
    if let Some(e) = s.model.result(feature).and_then(|r| r.error.clone()) {
        return Err(EngineError::Other(format!("{cmd}: {e}")));
    }
    let reply = MeshUnitsResult { feature, bodies, scale, units: to.into(), from: from.into(), param_warnings: Vec::new() };
    serde_json::to_value(reply).map_err(|e| EngineError::Other(e.to_string()))
}

/// The mesh body to convert: `body`, else the one selected body.
fn target(s: &Session, p: &Value, cmd: &str) -> Result<String> {
    if let Some(name) = str_(p, "body").filter(|b| !b.trim().is_empty()) {
        return Ok(name.to_owned());
    }
    let selected: Vec<&str> = s.selection.iter().filter_map(|x| if let Sel::Body { name } = x { Some(name.as_str()) } else { None }).collect();
    match selected.as_slice() {
        [name] => Ok((*name).to_owned()),
        [] => Err(bad(cmd, "select a mesh body")),
        _ => Err(bad(cmd, "select one mesh body")),
    }
}

fn convert(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "mesh.convert";
    let state = s.model.state();
    let given = target(s, p, cmd)?;
    // The selection and the Browser name a body of a further placement as the world shows it
    // (`Mesh (Scan:2)`): that name means the body it shows, which the conversion edits in its
    // component, as `appearance.assign` reads it (#9740).
    let body = s.own_body_name(&given).ok_or_else(|| bad(cmd, format!("no body `{given}`")))?;
    let source = state.body(&body).ok_or_else(|| bad(cmd, format!("no body `{given}`")))?;
    if !source.body.is_mesh() {
        return Err(bad(cmd, "select a mesh body"));
    }
    let owner = s.doc.model_body_component(source);
    if !s.doc.component_within(owner, s.active_component) {
        let name = s.doc.components.iter().find(|c| c.id == owner).map(|c| c.name.as_str()).unwrap_or(s.doc.name.as_str());
        return Err(bad(cmd, format!("activate component '{name}' first")));
    }
    let tolerance = expr(p, "tolerance").unwrap_or_else(|| "0.05 mm".into());
    check_expr(s, &tolerance, Kind::Length, cmd, "tolerance")?;
    // Conversion edits the component definition, independent of its occurrence placement.
    let active_component = std::mem::replace(&mut s.active_component, owner);
    let active_occurrence_path = s.active_occurrence_path.take();
    let result = add_feature(s, p, FeatureKind::MeshConvert { body: body.into(), tolerance });
    s.active_component = active_component;
    s.active_occurrence_path = active_occurrence_path;
    result
}
