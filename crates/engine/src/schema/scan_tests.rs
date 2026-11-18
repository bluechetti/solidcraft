use serde_json::{Value, json};

use super::{
    ScanAddFaceParams, ScanBasesParams, ScanBasesResult, ScanCloseSurfaceParams, ScanCompleteParams, ScanCurveParams, ScanDeviationParams,
    ScanDeviationResult, ScanEditNetParams, ScanEditSurfaceParams, ScanFitParams, ScanFitResult, ScanFitSurfaceParams, ScanFitSurfaceResult,
    ScanFloatingParams, ScanFreeFormParams, ScanHolesParams, ScanMergeSurfacesParams, ScanMirrorFreeFormParams, ScanPlaceParams, ScanPlaceResult,
    ScanReduceParams, ScanRepairParams, ScanSectionParams, ScanSectionsAlongParams, ScanSliceSurfaceParams, ScanSquareParams, ScanStandParams,
    ScanSymmetryParams, ScanSymmetryResult, ScanWrapSurfaceParams,
};
use crate::Session;
use crate::schema::tests::{assert_keys, assert_strict_keeps_todays_errors, conforms};
use crate::schema::{ParamCheck, input_schema_for, output_schema_for};

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<ScanBasesParams>(), &["body"]);
    assert_keys(input_schema_for::<ScanPlaceParams>(), &["body"]);
    assert_keys(input_schema_for::<ScanStandParams>(), &["base", "body", "point", "points"]);
    assert_keys(input_schema_for::<ScanSymmetryParams>(), &["body", "guide", "keep", "plane"]);
    assert_keys(input_schema_for::<ScanSquareParams>(), &["body", "guide", "plane"]);
    assert_keys(input_schema_for::<ScanFitParams>(), &["body", "keep", "make", "point", "radius", "saved", "tolerance", "triangles"]);
    assert_keys(input_schema_for::<ScanDeviationParams>(), &["auto_limit", "bands", "body", "limit", "pins", "scan", "show", "tolerance"]);
    assert_keys(
        input_schema_for::<ScanFitSurfaceParams>(),
        &["body", "controls", "point", "radius", "saved", "smoothing", "tolerance", "triangles", "trim", "whole_face"],
    );
    assert_keys(input_schema_for::<ScanCompleteParams>(), &["body", "guide", "plane", "reach"]);
    assert_keys(input_schema_for::<ScanFloatingParams>(), &["below", "body", "in_place", "keep", "remove"]);
    assert_keys(input_schema_for::<ScanFreeFormParams>(), &["body", "corners", "count", "curves", "fit", "plane", "reach", "stiffness"]);
    assert_keys(input_schema_for::<ScanEditNetParams>(), &["after", "distance", "in", "line", "op", "surface"]);
    assert_keys(
        input_schema_for::<ScanWrapSurfaceParams>(),
        &["arc", "axis", "body", "details", "grow", "point", "reach", "saved", "stiffness", "triangles"],
    );
    assert_keys(input_schema_for::<ScanAddFaceParams>(), &["body", "edge", "far", "reach", "rows", "surface", "symmetry"]);
    assert_keys(input_schema_for::<ScanMirrorFreeFormParams>(), &["gap", "plane", "surface"]);
    assert_keys(input_schema_for::<ScanMergeSurfacesParams>(), &["gap", "stitch", "surfaces"]);
    assert_keys(input_schema_for::<ScanCloseSurfaceParams>(), &["close", "continuity", "surface"]);
    assert_keys(input_schema_for::<ScanSliceSurfaceParams>(), &["cap", "keep", "plane", "surface"]);
    assert_keys(input_schema_for::<ScanHolesParams>(), &["at", "body", "fair", "in_place", "max_edges"]);
    assert_keys(input_schema_for::<ScanRepairParams>(), &["body", "crossings", "in_place"]);
    assert_keys(input_schema_for::<ScanSectionParams>(), &["body", "dimensions", "offset", "plane", "tolerance"]);
    assert_keys(input_schema_for::<ScanSectionsAlongParams>(), &["body", "count", "curves", "every", "sketch", "tolerance"]);
    assert_keys(
        input_schema_for::<ScanReduceParams>(),
        &["area", "body", "grow", "in_place", "keep_rim", "max_error", "point", "radius", "ratio", "saved", "selected", "triangles"],
    );
    assert_keys(input_schema_for::<ScanCurveParams>(), &["body", "closed", "points", "sketch", "tolerance"]);
    assert_keys(
        input_schema_for::<ScanEditSurfaceParams>(),
        &[
            "brush",
            "crease",
            "delete_face",
            "dots",
            "faces",
            "falloff",
            "hold",
            "keep_half",
            "kept_edges",
            "moves",
            "neighbours",
            "net",
            "points",
            "reach",
            "refit",
            "scan",
            "slide",
            "smooth",
            "surface",
            "symmetry",
            "uncrease",
            "weld",
        ],
    );
}

#[test]
fn strict_keeps_todays_errors() {
    assert_strict_keeps_todays_errors(&[
        ("scan.stand_on_base", json!({"point": "here"})),
        ("scan.symmetry", json!({"guide": {"from": [0, 0, 0]}})),
        ("scan.square_to_symmetry", json!({"guide": {"from": [0, 0, 0], "to": "there"}})),
        ("scan.fit_plane", json!({"point": "here"})),
    ]);
}

/// Every form of the scan commands, in Warn: each reply matches its output schema and carries no
/// warning.
#[test]
fn replies_match_the_output_schema() {
    let dir = std::env::temp_dir().join(format!("solidcraft-scan-schema-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("block.stl");
    // A block 30 × 20 × 10, each side cut into 24 × 24 squares: enough points to look for a mirror plane.
    let (size, n) = ([30.0f64, 20.0, 10.0], 24u32);
    let mut stl = String::from("solid b\n");
    for axis in 0..3 {
        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
        for high in [false, true] {
            let at = |i: u32, j: u32| {
                let mut c = [0.0, 0.0, 5.0];
                if high {
                    c[axis] += size[axis];
                }
                c[u] += size[u] * f64::from(i) / f64::from(n);
                c[v] += size[v] * f64::from(j) / f64::from(n);
                c
            };
            for i in 0..n {
                for j in 0..n {
                    for tri in [[at(i, j), at(i + 1, j), at(i + 1, j + 1)], [at(i, j), at(i + 1, j + 1), at(i, j + 1)]] {
                        stl += "facet normal 0 0 0\nouter loop\n";
                        for [x, y, z] in tri {
                            stl += &format!("vertex {x} {y} {z}\n");
                        }
                        stl += "endloop\nendfacet\n";
                    }
                }
            }
        }
    }
    std::fs::write(&path, stl + "endsolid b\n").unwrap();

    let mut s = Session { param_check: ParamCheck::Warn, ..Session::default() };
    s.execute("file.insert_mesh", &json!({"path": path.to_string_lossy()})).unwrap();
    let mut run = |id: &str, p: Value| s.execute(id, &p).unwrap_or_else(|e| panic!("{id} {p}: {e}"));
    let check = |schema: schemars::Schema, reply: &Value| {
        let schema = schema.as_value();
        assert!(conforms(schema, schema, reply), "{reply} doesn't match {schema}");
        assert!(reply.get("param_warnings").is_none(), "{reply}");
    };
    check(output_schema_for::<ScanBasesResult>(), &run("scan.bases", json!({})));
    for (id, p) in [
        ("scan.rest_on_grid", json!({})),
        ("scan.rest_on_grid", json!({})),
        ("scan.center", json!({})),
        ("scan.quick_align", json!({})),
        ("scan.stand_on_base", json!({"base": 1})),
        ("scan.stand_on_base", json!({"point": [15, 10, 0]})),
    ] {
        check(output_schema_for::<ScanPlaceResult>(), &run(id, p));
    }
    // The block is a mirror image about three planes (guides are tested with the commands).
    check(output_schema_for::<ScanSymmetryResult>(), &run("scan.symmetry", json!({})));
    check(output_schema_for::<ScanSymmetryResult>(), &run("scan.symmetry", json!({"keep": false})));
    check(output_schema_for::<ScanPlaceResult>(), &run("scan.square_to_symmetry", json!({})));
    // A point on a side of the block, wherever the moves left it: the anchor of its steadiest base.
    let on = run("scan.bases", json!({}))["bases"][0]["anchor"].clone();
    check(output_schema_for::<ScanFitResult>(), &run("scan.fit_plane", json!({"point": on})));
    check(output_schema_for::<ScanFitResult>(), &run("scan.fit_plane", json!({"point": on, "keep": false, "tolerance": "0.5 mm"})));
    check(output_schema_for::<ScanPlaceResult>(), &run("scan.stand_on_base", json!({})));
    check(output_schema_for::<ScanPlaceResult>(), &run("scan.stand_on_base", json!({})));
    let on = run("scan.bases", json!({}))["bases"][0]["anchor"].clone();
    check(output_schema_for::<ScanFitSurfaceResult>(), &run("scan.fit_surface", json!({"point": on, "radius": 4, "controls": 4, "smoothing": 0.5})));
    // Against a box where the block was made, before any of the moves above.
    run("solid.box", json!({"body_name": "Model", "operation": "new", "length": 30, "width": 20, "height": 10}));
    check(output_schema_for::<ScanDeviationResult>(), &run("scan.deviation", json!({"body": "Model", "limit": 100})));
    check(output_schema_for::<ScanDeviationResult>(), &run("scan.deviation", json!({"show": false})));
}
