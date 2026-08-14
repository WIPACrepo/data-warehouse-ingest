use crate::error::AppError;

#[test]
fn test_app_error_debug() {
    // Test that AppError implements Debug
    fn assert_debug<T: std::fmt::Debug>() {}
    assert_debug::<AppError>();
}
