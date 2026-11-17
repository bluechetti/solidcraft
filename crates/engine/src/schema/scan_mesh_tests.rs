use super::{ScanDenoiseParams, ScanMeshFromPointsParams, ScanRegisterParams, ScanRemeshParams};
use crate::schema::input_schema_for;
use crate::schema::tests::assert_keys;

#[test]
fn schemas_and_the_key_check_agree() {
    assert_keys(input_schema_for::<ScanRegisterParams>(), &["body", "ignored", "search", "to"]);
    assert_keys(input_schema_for::<ScanMeshFromPointsParams>(), &["cloud", "depth", "name", "trim"]);
    assert_keys(
        input_schema_for::<ScanRemeshParams>(),
        &["area", "body", "cell", "grow", "in_place", "point", "radius", "saved", "selected", "sharp", "with"],
    );
    assert_keys(input_schema_for::<ScanDenoiseParams>(), &["area", "body", "grow", "in_place", "point", "radius", "saved", "selected", "strength"]);
}
