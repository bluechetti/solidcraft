use super::{ScanCutItParams, ScanExtrudeItParams, ScanFindAllParams, ScanPatternItParams, ScanReconstructParams, ScanRevolveItParams};
use crate::schema::input_schema_for;
use crate::schema::tests::assert_keys;

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<ScanFindAllParams>(), &["body", "show"]);
    assert_keys(input_schema_for::<ScanReconstructParams>(), &["body"]);
    assert_keys(input_schema_for::<ScanExtrudeItParams>(), &["body", "into", "prism"]);
    assert_keys(input_schema_for::<ScanRevolveItParams>(), &["body", "into"]);
    assert_keys(input_schema_for::<ScanCutItParams>(), &["body", "hole", "into"]);
    assert_keys(input_schema_for::<ScanPatternItParams>(), &["body", "feature", "hole"]);
}
