#[tokio::test]
async fn test_shutdown_signal() {
    // This test just verifies the function exists and can be called
    // Actually testing the signal would require sending a real signal
    let shutdown_handle = tokio::spawn(crate::config::shutdown_signal());

    // Give it a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    // The task should still be running ( (waiting for signal)
    assert!(!shutdown_handle.is_finished());

    // Abort the task since we can't send a real signal in tests
    shutdown_handle.abort();
}
