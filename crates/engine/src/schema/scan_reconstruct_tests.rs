use super::{
    ScanCutItParams, ScanExtrudeItParams, ScanFilletItParams, ScanFindAllParams, ScanHoleItParams, ScanPatternItParams, ScanReconstructParams,
    ScanReconstructSelectionParams, ScanRevolveItParams, ScanShapeItParams, ScanShellItParams,
};
use crate::schema::input_schema_for;
use crate::schema::tests::assert_keys;

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<ScanFindAllParams>(), &["body", "show", "tolerance"]);
    assert_keys(input_schema_for::<ScanReconstructParams>(), &["body", "selected", "tolerance"]);
    assert_keys(input_schema_for::<ScanReconstructSelectionParams>(), &["body"]);
    assert_keys(input_schema_for::<ScanExtrudeItParams>(), &["body", "into", "prism", "selected"]);
    assert_keys(input_schema_for::<ScanRevolveItParams>(), &["body", "into", "selected"]);
    assert_keys(input_schema_for::<ScanCutItParams>(), &["body", "hole", "into", "selected"]);
    assert_keys(input_schema_for::<ScanShapeItParams>(), &["body", "into", "selected"]);
    assert_keys(input_schema_for::<ScanHoleItParams>(), &["body", "hole", "into", "selected", "thread"]);
    assert_keys(input_schema_for::<ScanPatternItParams>(), &["body", "feature", "hole", "selected"]);
    assert_keys(input_schema_for::<ScanFilletItParams>(), &["body", "into", "selected"]);
    assert_keys(input_schema_for::<ScanShellItParams>(), &["body", "into", "selected"]);
}
