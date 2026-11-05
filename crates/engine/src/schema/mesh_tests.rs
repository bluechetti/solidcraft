use serde_json::json;

use super::{MeshUnitsParams, MeshUnitsResult};
use crate::Session;
use crate::schema::tests::{assert_keys, assert_strict_keeps_todays_errors, conforms};
use crate::schema::{ParamCheck, input_schema_for, output_schema_for};

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<MeshUnitsParams>(), &["body", "from", "units"]);
}

#[test]
fn strict_keeps_todays_errors() {
    assert_strict_keeps_todays_errors(&[("mesh.units", json!({"units": "furlong"})), ("mesh.units", json!({"units": "m", "from": "yd"}))]);
}

/// A triangle 0.01 across, from an STL with no units, read again in metres: each form the command
/// takes, in Warn, matches the output schema with nothing to warn about.
#[test]
fn replies_match_the_output_schema() {
    let schema = output_schema_for::<MeshUnitsResult>();
    let schema = schema.as_value();
    let dir = std::env::temp_dir().join(format!("solidcraft-mesh-units-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tri.stl");
    std::fs::write(&path, "solid t\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 0.01 0 0\nvertex 0 0.01 0\nendloop\nendfacet\nendsolid t\n")
        .unwrap();
    let mut s = Session::default();
    s.execute("file.insert_mesh", &json!({"path": path.to_string_lossy()})).unwrap();
    s.param_check = ParamCheck::Warn;
    for p in [json!({"units": "m"}), json!({"units": "MM", "from": "m", "body": "tri"}), json!({"units": "in"})] {
        let reply = s.execute("mesh.units", &p).unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(reply.get("param_warnings").is_none() && conforms(schema, schema, &reply), "{p}: {reply} doesn't match {schema}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
