#[test]
fn test_logging_setup_function_exists() {
    // Compile-time check that the function exists with the right signature
    let _func: fn(&str) = crate::config::logging_setup;
}

#[test]
fn test_shutdown_signal_function_exists() {
    // Compile-time check that the function exists with the right signature
    let _ = crate::config::shutdown_signal;
}
