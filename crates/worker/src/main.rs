//! localai-worker binary (spec 04 O8).
//!
//! One-shot worker process:
//! 1. Read JSON payload from stdin: {job_id, kind, args}
//! 2. Dispatch to registered handler based on kind
//! 3. Emit result JSON to stdout: {version, job_id, status, result|error, provenance, cost_tokens}
//! 4. Exit with code 0 (valid result JSON, even if status=failed)
//!
//! In Phase 2, each kind dispatches to a real handler. For now, all handlers return
//! "not implemented" errors.

use localai_worker::{run_worker, WorkerError, WorkerExecError, WorkerPayload, WorkerResult};
use std::io::{self, Read};

/// Exit code for memory limit breach (spec 04 O7).
pub const EXIT_MEM_LIMIT: i32 = 137;

/// Registered handlers by job kind (phase 2: real implementations).
fn get_handler(kind: &str) -> fn(WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    match kind {
        "scrape" => handle_scrape,
        "ingest" => handle_ingest,
        "distill" => handle_distill,
        "agent" => handle_agent,
        "reembed" => handle_reembed,
        "maintenance" => handle_maintenance,
        _ => handle_unknown,
    }
}

/// Phase 2: real scrape handler.
fn handle_scrape(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler(
        "not implemented: scrape".to_string(),
    ))
}

/// Phase 2: real ingest handler.
fn handle_ingest(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler(
        "not implemented: ingest".to_string(),
    ))
}

/// Phase 2: real distill handler.
fn handle_distill(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler(
        "not implemented: distill".to_string(),
    ))
}

/// Phase 2: real agent handler.
fn handle_agent(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler(
        "not implemented: agent".to_string(),
    ))
}

/// Phase 2: real reembed handler.
fn handle_reembed(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler(
        "not implemented: reembed".to_string(),
    ))
}

/// Phase 2: real maintenance handler.
fn handle_maintenance(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler(
        "not implemented: maintenance".to_string(),
    ))
}

/// Unknown handler (unsupported kind).
fn handle_unknown(_payload: WorkerPayload) -> Result<serde_json::Value, WorkerExecError> {
    Err(WorkerExecError::Handler("unknown job kind".to_string()))
}

/// Read stdin and emit JSON result to stdout (spec 04 O8).
/// Always exits with 0 if valid result JSON was emitted, even on error.
fn main() {
    // Read payload from stdin.
    let mut input = String::new();
    if io::stdin().read_to_string(&mut input).is_err() {
        // Input read error → emit failed result with parse class.
        let result = WorkerResult::failed(
            0,
            WorkerError::new(
                "failed to read stdin",
                Some(localai_core::ErrorClass::Input),
            ),
            // Fail-safe: kind is unknown here — never default to trust.
            localai_core::Provenance::Untrusted,
        );
        if let Ok(json) = serde_json::to_string(&result) {
            println!("{}", json);
        }
        std::process::exit(0);
    }

    // Parse payload.
    let payload: WorkerPayload = match serde_json::from_str(&input) {
        Ok(p) => p,
        Err(e) => {
            // Parse failure → emit failed result with parse class (O8).
            let result = WorkerResult::failed(
                0,
                WorkerError::new(
                    format!("parse error: {}", e),
                    Some(localai_core::ErrorClass::Input),
                ),
                // Fail-safe: payload never parsed, kind unknown — Untrusted.
                // The message also echoes external input fragments.
                localai_core::Provenance::Untrusted,
            );
            if let Ok(json) = serde_json::to_string(&result) {
                println!("{}", json);
            }
            std::process::exit(0);
        }
    };

    // Dispatch to handler.
    let handler = get_handler(&payload.kind);
    let result = run_worker(payload, handler);

    // Emit result JSON.
    if let Ok(json) = serde_json::to_string(&result) {
        println!("{}", json);
    }

    std::process::exit(0);
}
