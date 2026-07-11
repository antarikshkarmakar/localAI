//! Integration tests for heartbeat file reading.

use localai_watchdog::{Action, PolicyConfig, WatchdogPolicy};
use std::fs;
use tempfile::TempDir;

#[test]
fn test_heartbeat_file_missing() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let heartbeat_path = temp_dir.path().join("heartbeat");

    // Simulate reading a missing file as None
    let counter = if heartbeat_path.exists() {
        fs::read_to_string(&heartbeat_path)
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
    } else {
        None
    };

    assert_eq!(counter, None, "missing heartbeat file should be None");
}

#[test]
fn test_heartbeat_file_write_and_read() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let heartbeat_path = temp_dir.path().join("heartbeat");

    // Write counter 42
    fs::write(&heartbeat_path, "42").expect("failed to write heartbeat");

    // Read it back
    let counter = fs::read_to_string(&heartbeat_path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());

    assert_eq!(counter, Some(42), "should read back the counter value");
}

#[test]
fn test_heartbeat_corruption_non_numeric() {
    let temp_dir = TempDir::new().expect("failed to create temp dir");
    let heartbeat_path = temp_dir.path().join("heartbeat");

    // Write garbage
    fs::write(&heartbeat_path, "garbage").expect("failed to write");

    // Try to parse
    let counter = fs::read_to_string(&heartbeat_path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());

    assert_eq!(counter, None, "corrupted heartbeat should be None");
}

#[test]
fn test_full_cycle_with_policy() {
    let config = PolicyConfig {
        max_missed_polls: 2,
        backoff_initial_ms: 100,
        backoff_max_ms: 1000,
        backoff_multiplier: 2.0,
    };

    let mut policy = WatchdogPolicy::new(config);

    // Simulate a full cycle: startup → healthy → stall → restart → recovery
    assert_eq!(policy.observe(Some(1)), Action::None);
    assert_eq!(policy.observe(Some(2)), Action::None);
    assert_eq!(policy.observe(Some(3)), Action::None);

    // Stall
    assert_eq!(policy.observe(Some(3)), Action::None); // first miss
    assert_eq!(policy.observe(Some(3)), Action::Restart); // triggers

    // After restart
    policy.acknowledge_restart();
    assert_eq!(policy.observe(Some(1)), Action::None); // counter resets
    assert_eq!(policy.observe(Some(2)), Action::None);
}

#[test]
fn test_crash_loop_escalation() {
    let config = PolicyConfig {
        max_missed_polls: 1,
        backoff_initial_ms: 100,
        backoff_max_ms: 5000,
        backoff_multiplier: 2.0,
    };

    let mut policy = WatchdogPolicy::new(config);

    // First crash
    policy.observe(Some(1));
    assert_eq!(policy.observe(Some(1)), Action::Restart);
    assert_eq!(policy.current_backoff_ms(), 100);
    policy.acknowledge_restart();

    // Crash immediately again
    policy.observe(Some(1));
    assert_eq!(policy.observe(Some(1)), Action::Restart);
    assert_eq!(policy.current_backoff_ms(), 200);
    policy.acknowledge_restart();

    // Crash again
    policy.observe(Some(1));
    assert_eq!(policy.observe(Some(1)), Action::Restart);
    assert_eq!(policy.current_backoff_ms(), 400);
}
