use super::{
    ScanCutItParams, ScanExtrudeItParams, ScanFilletItParams, ScanFindAllParams, ScanHoleItParams, ScanNestParams, ScanPatternItParams,
    ScanReconstructParams, ScanReconstructSelectionParams, ScanRestParams, ScanRevolveItParams, ScanShapeItParams, ScanShellItParams,
    ScanToCadParams,
};
use crate::schema::input_schema_for;
use crate::schema::tests::assert_keys;

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<ScanFindAllParams>(), &["body", "limit", "show", "tolerance"]);
    assert_keys(input_schema_for::<ScanReconstructParams>(), &["body", "selected", "tolerance"]);
    assert_keys(input_schema_for::<ScanReconstructSelectionParams>(), &["body"]);
    assert_keys(input_schema_for::<ScanExtrudeItParams>(), &["body", "into", "pattern", "prism", "selected"]);
    assert_keys(input_schema_for::<ScanRevolveItParams>(), &["body", "into", "selected", "wall"]);
    assert_keys(input_schema_for::<ScanCutItParams>(), &["body", "hole", "into", "selected"]);
    assert_keys(input_schema_for::<ScanShapeItParams>(), &["body", "into", "selected"]);
    assert_keys(input_schema_for::<ScanRestParams>(), &["body", "rounds"]);
    assert_keys(input_schema_for::<ScanToCadParams>(), &["body", "clean", "rest", "scan", "tolerance"]);
    assert_keys(input_schema_for::<ScanHoleItParams>(), &["body", "hole", "into", "selected", "thread"]);
    assert_keys(input_schema_for::<ScanNestParams>(), &["allowance", "body", "clearance", "deep", "margin", "prism", "selected", "turned"]);
    assert_keys(input_schema_for::<ScanPatternItParams>(), &["body", "feature", "hole", "prism", "selected"]);
    assert_keys(input_schema_for::<ScanFilletItParams>(), &["body", "into", "selected"]);
    assert_keys(input_schema_for::<ScanShellItParams>(), &["body", "into", "selected"]);
}
