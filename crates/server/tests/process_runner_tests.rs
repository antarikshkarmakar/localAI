//! Tests for ProcessRunner (spec 04 O8-O11).
//!
//! Uses fake shell-script workers to avoid depending on localai-worker binary.
//! Each test case spawns a temporary script, verifies ProcessRunner handles it correctly.
//!
//! Integration tests are their own crate — clippy's allow-*-in-tests config
//! doesn't reach helper fns here; allow test-only lints file-wide.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use localai_core::ErrorClass;
use localai_server::supervisor::JobRunner;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

/// Create a fake shell worker at a temp path that emits a specific JSON result.
/// Returns the temp dir (must be kept alive for the test) and the script path.
fn make_fake_worker(output_json: &str) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("fake_worker.sh");

    let script_content = format!(
        r#"#!/bin/sh
# Fake worker: consume stdin, emit the given JSON
cat > /dev/null
echo '{}'
exit 0
"#,
        output_json.replace('\'', "'\\''")
    );

    let mut file = fs::File::create(&script_path).expect("create script");
    file.write_all(script_content.as_bytes())
        .expect("write script");
    drop(file);

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod +x");
    }

    (dir, script_path)
}

/// Helper: create a fake worker that sleeps before exiting (for timeout tests).
fn make_sleeping_worker(sleep_secs: u64) -> (TempDir, PathBuf) {
    let dir = TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("fake_worker.sh");

    let script_content = format!(
        r#"#!/bin/sh
cat > /dev/null
sleep {}
exit 0
"#,
        sleep_secs
    );

    let mut file = fs::File::create(&script_path).expect("create script");
    file.write_all(script_content.as_bytes())
        .expect("write script");
    drop(file);

    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&script_path, perms).expect("chmod +x");
    }

    (dir, script_path)
}

#[test]
fn test_valid_done_result() {
    let json = r#"{"version":1,"job_id":42,"status":"done","result":{"output":"hello"},"error":null,"provenance":"System","cost_tokens":100,"cost_usd":0.05}"#;
    let (tempdir, worker_bin) = make_fake_worker(json);

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(5));

    let outcome = runner.run_sync("scrape", r#"{"url":"https://example.com"}"#);

    assert_eq!(outcome.status, localai_server::supervisor::DoneStatus::Done);
    assert!(outcome.result_json.is_some());
    assert_eq!(
        outcome.result_json.as_ref().unwrap(),
        r#"{"output":"hello"}"#
    );
    assert!(outcome.error.is_none());
    assert!(outcome.error_class.is_none());

    // Tempdir dropped here, cleanup happens.
    drop(tempdir);
}

#[test]
fn test_failed_with_input_class() {
    let json = r#"{"version":1,"job_id":1,"status":"failed","result":null,"error":{"class":"input","message":"bad URL format","retryable":false},"provenance":"Untrusted","cost_tokens":0,"cost_usd":0.0}"#;
    let (tempdir, worker_bin) = make_fake_worker(json);

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(5));

    let outcome = runner.run_sync("scrape", r#"{"url":"invalid"}"#);

    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Failed
    );
    assert!(outcome.error.is_some());
    assert!(outcome.error_class.is_some());
    assert_eq!(outcome.error_class.unwrap(), ErrorClass::Input);
    assert!(outcome.error.as_ref().unwrap().contains("bad URL"));

    drop(tempdir);
}

#[test]
fn test_garbage_output_maps_to_bug() {
    let (tempdir, worker_bin) = make_fake_worker("not valid json at all");

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(5));

    let outcome = runner.run_sync("distill", r#"{"data":"test"}"#);

    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Failed
    );
    assert!(outcome.error.is_some());
    assert_eq!(outcome.error_class, Some(ErrorClass::Bug));
    // Error should contain a snippet of the bad output
    let err_msg = outcome.error.as_ref().unwrap();
    assert!(err_msg.contains("not valid json"));

    drop(tempdir);
}

#[test]
#[cfg(unix)]
fn test_timeout_kills_child() {
    // Worker sleeps 10 seconds, but timeout is 1 second.
    let (tempdir, worker_bin) = make_sleeping_worker(10);

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(1));

    let start = std::time::Instant::now();
    let outcome = runner.run_sync("maintenance", r#"{}"#);
    let elapsed = start.elapsed();

    // Should timeout and kill the child promptly (well before 10s).
    assert!(elapsed < Duration::from_secs(5), "should timeout quickly");
    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Failed
    );
    assert_eq!(outcome.error_class, Some(ErrorClass::Transient));
    assert!(outcome
        .error
        .as_ref()
        .unwrap()
        .to_lowercase()
        .contains("timeout"));

    // Process should be dead by now; tempdir dropped for cleanup.
    drop(tempdir);
}

#[test]
fn test_nonexistent_worker_bin() {
    let nonexistent = PathBuf::from("/nonexistent/fake/worker/binary");

    let runner =
        localai_server::process_runner::ProcessRunner::new(nonexistent, Duration::from_secs(5));

    let outcome = runner.run_sync("scrape", r#"{"url":"test"}"#);

    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Failed
    );
    assert_eq!(outcome.error_class, Some(ErrorClass::Bug));
    // Should not panic, error should be in the message.
    assert!(outcome.error.is_some());

    // Drop the outcome to verify no panic unwinding.
    drop(outcome);
}

#[test]
fn test_partial_status() {
    let json = r#"{"version":1,"job_id":5,"status":"partial","result":{"partial":"data"},"error":{"class":"transient","message":"recovered partially","retryable":true},"provenance":"VerifiedKb","cost_tokens":50,"cost_usd":0.01}"#;
    let (tempdir, worker_bin) = make_fake_worker(json);

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(5));

    let outcome = runner.run_sync("ingest", r#"{"file":"data.txt"}"#);

    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Partial
    );
    assert!(outcome.result_json.is_some());
    assert!(outcome.error.is_some());
    // Partial results can have errors but should map ErrorClass::Transient correctly.
    // (Partial status itself doesn't define error_class in supervisor—the runner decides).

    drop(tempdir);
}

#[test]
fn test_worker_unknown_error_class_defaults_to_bug() {
    // Schema allows omitting error.class; RunOutcome should treat as Bug.
    let json = r#"{"version":1,"job_id":10,"status":"failed","result":null,"error":{"message":"something broke"},"provenance":"System","cost_tokens":0,"cost_usd":0.0}"#;
    let (tempdir, worker_bin) = make_fake_worker(json);

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(5));

    let outcome = runner.run_sync("distill", r#"{}"#);

    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Failed
    );
    assert_eq!(outcome.error_class, Some(ErrorClass::Bug));

    drop(tempdir);
}

#[test]
fn test_resource_error_class() {
    let json = r#"{"version":1,"job_id":15,"status":"failed","result":null,"error":{"class":"resource","message":"OOM","retryable":true},"provenance":"Untrusted","cost_tokens":0,"cost_usd":0.0}"#;
    let (tempdir, worker_bin) = make_fake_worker(json);

    let runner =
        localai_server::process_runner::ProcessRunner::new(worker_bin, Duration::from_secs(5));

    let outcome = runner.run_sync("agent", r#"{"prompt":"test"}"#);

    assert_eq!(
        outcome.status,
        localai_server::supervisor::DoneStatus::Failed
    );
    assert_eq!(outcome.error_class, Some(ErrorClass::Resource));

    drop(tempdir);
}
