use super::*;

#[test]
fn caller_selected_legacy_fixture_text_rotation_and_preservation() {
    check_native_fixture("RPSHOP_NATIVE_FIXTURE", true);
}

#[test]
fn caller_selected_current_fixture_text_rotation_and_preservation() {
    check_native_fixture("RPSHOP_NATIVE_V11_FIXTURE", false);
}
