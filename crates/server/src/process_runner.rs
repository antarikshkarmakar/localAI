//! ProcessRunner: bridge between supervisor and child worker processes (spec 04 O8-O11).
//!
//! One-shot worker lifecycle:
//! 1. Spawn worker binary with std::process::Command
//! 2. Write WorkerPayload JSON to stdin: {job_id: 0, kind, args}
//! 3. Read WorkerResult JSON from stdout with timeout
//! 4. Parse result; on timeout/error, map to RunOutcome with ErrorClass
//! 5. Kill child if timeout (SIGKILL via drop after std::process::Command timeout)

use crate::supervisor::{DoneStatus, JobRunner, RunOutcome};
use localai_core::ErrorClass;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tracing::{debug, error, warn};

/// Wire format for worker payload (spec 04 O8).
/// Matches the contract in localai-worker/src/lib.rs WorkerPayload.
#[derive(Debug, Serialize, Deserialize)]
struct WorkerPayload {
    job_id: i64,
    kind: String,
    args: JsonValue,
}

/// Wire format for worker result (spec 04 O8, schemas/worker-result.schema.json).
/// Minimal local mirror; do NOT import from localai-worker (dependency direction).
#[derive(Debug, Serialize, Deserialize, Clone)]
struct WorkerResult {
    version: i32,
    job_id: i64,
    status: String,
    result: Option<JsonValue>,
    error: Option<WorkerError>,
    provenance: String,
    cost_tokens: i64,
    cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    artifacts_dir: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct WorkerError {
    #[serde(skip_serializing_if = "Option::is_none")]
    class: Option<String>,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
}

/// ProcessRunner spawns a standalone worker binary for each job (spec 04 O8-O11).
/// Implements JobRunner trait; run_sync is called via spawn_blocking by Supervisor.
pub struct ProcessRunner {
    /// Path to the worker binary (crates/worker/target/.../localai-worker or test fake).
    worker_bin: PathBuf,
    /// Wall-clock timeout for job execution (spec 04 O10).
    timeout: Duration,
}

impl ProcessRunner {
    pub fn new(worker_bin: PathBuf, timeout: Duration) -> Self {
        Self {
            worker_bin,
            timeout,
        }
    }
}

impl JobRunner for ProcessRunner {
    fn run_sync(&self, kind: &str, payload: &str) -> RunOutcome {
        // Parse the incoming payload string as JSON (the args value).
        let args = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(e) => {
                debug!("job payload parse failed: {}", e);
                return RunOutcome {
                    status: DoneStatus::Failed,
                    result_json: None,
                    error: Some(format!("payload parse error: {}", e)),
                    error_class: Some(ErrorClass::Input),
                };
            }
        };

        // Build the worker payload: job_id=0 (not threaded; see O8 docs), kind, args.
        let worker_payload = WorkerPayload {
            job_id: 0, // O8: job_id is set by caller if needed; defaults to 0.
            kind: kind.to_string(),
            args,
        };

        let payload_json = match serde_json::to_string(&worker_payload) {
            Ok(s) => s,
            Err(e) => {
                error!("failed to serialize worker payload: {}", e);
                return RunOutcome {
                    status: DoneStatus::Failed,
                    result_json: None,
                    error: Some(format!("internal: payload serialization failed: {}", e)),
                    error_class: Some(ErrorClass::Bug),
                };
            }
        };

        // Spawn the worker process.
        let mut child = match Command::new(&self.worker_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                error!("failed to spawn worker: {}", e);
                return RunOutcome {
                    status: DoneStatus::Failed,
                    result_json: None,
                    error: Some(format!(
                        "worker spawn failed ({}): {}",
                        self.worker_bin.display(),
                        e
                    )),
                    error_class: Some(ErrorClass::Bug),
                };
            }
        };

        // Write payload to stdin.
        {
            let stdin = match child.stdin.as_mut() {
                Some(s) => s,
                None => {
                    error!("stdin not piped");
                    return RunOutcome {
                        status: DoneStatus::Failed,
                        result_json: None,
                        error: Some("worker stdin not available".to_string()),
                        error_class: Some(ErrorClass::Bug),
                    };
                }
            };

            if let Err(e) = stdin.write_all(payload_json.as_bytes()) {
                error!("failed to write to worker stdin: {}", e);
                let _ = child.kill();
                return RunOutcome {
                    status: DoneStatus::Failed,
                    result_json: None,
                    error: Some(format!("stdin write failed: {}", e)),
                    error_class: Some(ErrorClass::Bug),
                };
            }
        }

        // Drop stdin handle so worker sees EOF.
        drop(child.stdin.take());

        // Read stdout with timeout (spec 04 O10).
        // Use wait_with_output() with wait_timeout pattern.
        let start = std::time::Instant::now();
        let timeout = self.timeout;

        loop {
            match child.try_wait() {
                Ok(Some(_status)) => {
                    // Child exited; collect output.
                    let output = match child.wait_with_output() {
                        Ok(o) => o,
                        Err(e) => {
                            error!("failed to collect output: {}", e);
                            return RunOutcome {
                                status: DoneStatus::Failed,
                                result_json: None,
                                error: Some(format!("output collection failed: {}", e)),
                                error_class: Some(ErrorClass::Bug),
                            };
                        }
                    };

                    let stdout_str = String::from_utf8_lossy(&output.stdout);
                    return parse_worker_result(&stdout_str, &output.stderr);
                }
                Ok(None) => {
                    // Child still running; check timeout.
                    if start.elapsed() > timeout {
                        warn!(
                            "worker timeout after {:?}s; killing (spec 04 O10)",
                            timeout.as_secs()
                        );
                        let _ = child.kill();
                        // Give it a moment to die; don't wait forever.
                        let _ = child.wait();
                        return RunOutcome {
                            status: DoneStatus::Failed,
                            result_json: None,
                            error: Some(format!(
                                "worker timeout after {}s (spec 04 O10)",
                                timeout.as_secs()
                            )),
                            error_class: Some(ErrorClass::Transient),
                        };
                    }
                    // Sleep a bit before polling again.
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(e) => {
                    error!("failed to poll child: {}", e);
                    return RunOutcome {
                        status: DoneStatus::Failed,
                        result_json: None,
                        error: Some(format!("child poll failed: {}", e)),
                        error_class: Some(ErrorClass::Bug),
                    };
                }
            }
        }
    }
}

/// Parse WorkerResult JSON from stdout, map to RunOutcome.
/// On parse error, constructs a Failed outcome with a snippet of the bad output.
fn parse_worker_result(stdout: &str, stderr: &[u8]) -> RunOutcome {
    let result: WorkerResult = match serde_json::from_str(stdout) {
        Ok(r) => r,
        Err(e) => {
            // Unparseable output → Bug class, include snippet.
            let snippet = if stdout.len() > 500 {
                format!("{}...", &stdout[..500])
            } else {
                stdout.to_string()
            };

            debug!("worker result parse failed: {}; stdout: {}", e, snippet);
            let stderr_hint = if !stderr.is_empty() {
                let stderr_str = String::from_utf8_lossy(stderr);
                format!(
                    "; stderr: {}",
                    &stderr_str[..std::cmp::min(200, stderr_str.len())]
                )
            } else {
                String::new()
            };

            return RunOutcome {
                status: DoneStatus::Failed,
                result_json: None,
                error: Some(format!(
                    "worker result parse error: {}; output: {}{}",
                    e, snippet, stderr_hint
                )),
                error_class: Some(ErrorClass::Bug),
            };
        }
    };

    // Map worker result status to RunOutcome.
    let status = match result.status.as_str() {
        "done" => DoneStatus::Done,
        "partial" => DoneStatus::Partial,
        "failed" => DoneStatus::Failed,
        _ => {
            warn!("unknown worker status: {}", result.status);
            DoneStatus::Failed
        }
    };

    // Extract error_class from error.class if present.
    let error_class = result.error.as_ref().and_then(|e| {
        e.class.as_deref().map(|class| match class {
            "transient" => ErrorClass::Transient,
            "input" => ErrorClass::Input,
            "bug" => ErrorClass::Bug,
            "resource" => ErrorClass::Resource,
            _ => {
                warn!("unknown error class: {}", class);
                ErrorClass::Bug
            }
        })
    });

    // If status is Failed and no error_class, default to Bug.
    let error_class = if status == DoneStatus::Failed && error_class.is_none() {
        Some(ErrorClass::Bug)
    } else {
        error_class
    };

    // Extract error message.
    let error_msg = result.error.as_ref().map(|e| e.message.clone());

    // Extract result JSON.
    let result_json = result.result.map(|v| v.to_string());

    RunOutcome {
        status,
        result_json,
        error: error_msg,
        error_class,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_worker_result_done() {
        let json = r#"{"version":1,"job_id":42,"status":"done","result":{"output":"hello"},"error":null,"provenance":"System","cost_tokens":100,"cost_usd":0.05}"#;
        let outcome = parse_worker_result(json, b"");

        assert_eq!(outcome.status, DoneStatus::Done);
        assert!(outcome.result_json.is_some());
        assert_eq!(
            outcome.result_json.as_ref().unwrap(),
            r#"{"output":"hello"}"#
        );
        assert!(outcome.error.is_none());
        assert!(outcome.error_class.is_none());
    }

    #[test]
    fn test_parse_worker_result_failed_with_class() {
        let json = r#"{"version":1,"job_id":1,"status":"failed","result":null,"error":{"class":"input","message":"bad input","retryable":false},"provenance":"Untrusted","cost_tokens":0,"cost_usd":0.0}"#;
        let outcome = parse_worker_result(json, b"");

        assert_eq!(outcome.status, DoneStatus::Failed);
        assert!(outcome.error.is_some());
        assert_eq!(outcome.error.as_ref().unwrap(), "bad input");
        assert_eq!(outcome.error_class, Some(ErrorClass::Input));
    }

    #[test]
    fn test_parse_worker_result_failed_no_class_defaults_to_bug() {
        let json = r#"{"version":1,"job_id":1,"status":"failed","result":null,"error":{"message":"something broke"},"provenance":"System","cost_tokens":0,"cost_usd":0.0}"#;
        let outcome = parse_worker_result(json, b"");

        assert_eq!(outcome.status, DoneStatus::Failed);
        assert_eq!(outcome.error_class, Some(ErrorClass::Bug));
    }

    #[test]
    fn test_parse_worker_result_garbage() {
        let bad_json = "not json at all";
        let outcome = parse_worker_result(bad_json, b"");

        assert_eq!(outcome.status, DoneStatus::Failed);
        assert_eq!(outcome.error_class, Some(ErrorClass::Bug));
        assert!(outcome.error.is_some());
        assert!(outcome
            .error
            .as_ref()
            .unwrap()
            .to_lowercase()
            .contains("parse error"));
    }

    #[test]
    fn test_parse_worker_result_partial() {
        let json = r#"{"version":1,"job_id":5,"status":"partial","result":{"partial":"data"},"error":{"class":"transient","message":"recovered","retryable":true},"provenance":"VerifiedKb","cost_tokens":50,"cost_usd":0.01}"#;
        let outcome = parse_worker_result(json, b"");

        assert_eq!(outcome.status, DoneStatus::Partial);
        assert!(outcome.result_json.is_some());
        assert!(outcome.error.is_some());
        assert_eq!(outcome.error_class, Some(ErrorClass::Transient));
    }

    #[test]
    fn test_parse_worker_result_resource_class() {
        let json = r#"{"version":1,"job_id":10,"status":"failed","result":null,"error":{"class":"resource","message":"OOM"},"provenance":"System","cost_tokens":0,"cost_usd":0.0}"#;
        let outcome = parse_worker_result(json, b"");

        assert_eq!(outcome.error_class, Some(ErrorClass::Resource));
    }

    #[test]
    fn test_worker_payload_serialization() {
        let payload = WorkerPayload {
            job_id: 42,
            kind: "scrape".to_string(),
            args: json!({"url": "https://example.com"}),
        };

        let json = serde_json::to_string(&payload).expect("serialize");
        let deserialized: WorkerPayload = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.job_id, 42);
        assert_eq!(deserialized.kind, "scrape");
        assert_eq!(deserialized.args["url"], "https://example.com");
    }
}
