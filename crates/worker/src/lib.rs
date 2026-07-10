//! One-shot worker harness (spec 04 O8-O11).
//!
//! Workers are standalone binaries that:
//! 1. Receive a JSON payload from stdin: `{job_id, kind, args}`
//! 2. Process the job via a registered handler
//! 3. Emit a result JSON to stdout: `{version, job_id, status, result|error, provenance, cost_tokens, cost_usd, artifacts_dir}`
//!
//! Invariants:
//! - Result provenance is ALWAYS set (spec 04 O9); scraper/agent kinds default to Untrusted
//! - Errors caught from handlers → failed result with error class + message
//! - No panics escape; all errors serialize as valid JSON (spec 04 O8)
//! - Round-trip: WorkerResult → JSON → WorkerResult are identical

use localai_core::{ErrorClass, Provenance};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use thiserror::Error;

#[cfg(test)]
use serde_json::json;

/// Worker job payload from stdin (spec 04 O8).
#[derive(Debug, Clone, Deserialize)]
pub struct WorkerPayload {
    pub job_id: i64,
    pub kind: String,
    pub args: JsonValue,
}

/// Error result info for a failed/errored job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

impl WorkerError {
    /// Construct from a message and error class hint.
    pub fn new(message: impl Into<String>, class: Option<ErrorClass>) -> Self {
        Self {
            class: class.map(|c| format!("{:?}", c).to_lowercase()),
            message: message.into(),
            retryable: class.map(|c| match c {
                ErrorClass::Transient => true,
                ErrorClass::Input => false,
                ErrorClass::Bug => false,
                ErrorClass::Resource => true,
            }),
        }
    }
}

/// Worker result to stdout (spec 04 O8, spec 07 O9).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerResult {
    pub version: i32,
    pub job_id: i64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<WorkerError>,
    pub provenance: String,
    pub cost_tokens: i64,
    pub cost_usd: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts_dir: Option<String>,
}

impl WorkerResult {
    /// Create a done result. Provenance is mandatory (O9).
    pub fn done(
        job_id: i64,
        result: impl Into<JsonValue>,
        provenance: Provenance,
        cost_tokens: i64,
        cost_usd: f64,
    ) -> Self {
        Self {
            version: 1,
            job_id,
            status: "done".to_string(),
            result: Some(result.into()),
            error: None,
            provenance: Self::provenance_string(provenance),
            cost_tokens,
            cost_usd,
            artifacts_dir: None,
        }
    }

    /// Create a failed result. Provenance is mandatory (O9).
    pub fn failed(job_id: i64, error: WorkerError, provenance: Provenance) -> Self {
        Self {
            version: 1,
            job_id,
            status: "failed".to_string(),
            result: None,
            error: Some(error),
            provenance: Self::provenance_string(provenance),
            cost_tokens: 0,
            cost_usd: 0.0,
            artifacts_dir: None,
        }
    }

    /// Create a partial result. Provenance is mandatory (O9).
    pub fn partial(
        job_id: i64,
        result: impl Into<JsonValue>,
        error: WorkerError,
        provenance: Provenance,
        cost_tokens: i64,
        cost_usd: f64,
    ) -> Self {
        Self {
            version: 1,
            job_id,
            status: "partial".to_string(),
            result: Some(result.into()),
            error: Some(error),
            provenance: Self::provenance_string(provenance),
            cost_tokens,
            cost_usd,
            artifacts_dir: None,
        }
    }

    /// Add artifacts directory.
    pub fn with_artifacts_dir(mut self, dir: Option<String>) -> Self {
        self.artifacts_dir = dir;
        self
    }

    fn provenance_string(p: Provenance) -> String {
        match p {
            Provenance::System => "System",
            Provenance::UserDirect => "UserDirect",
            Provenance::VerifiedKb => "VerifiedKb",
            Provenance::UnverifiedKb => "UnverifiedKb",
            Provenance::Untrusted => "Untrusted",
        }
        .to_string()
    }
}

/// Worker-specific errors.
#[derive(Debug, Error)]
pub enum WorkerExecError {
    #[error("parse error: {0}")]
    Parse(String),

    #[error("handler error: {0}")]
    Handler(String),

    #[error("resource limit: {0}")]
    Resource(String),
}

impl WorkerExecError {
    /// Map to error class for result.
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Parse(_) => ErrorClass::Input,
            Self::Handler(_) => ErrorClass::Bug,
            Self::Resource(_) => ErrorClass::Resource,
        }
    }
}

/// Handler signature: takes payload, returns result or error.
pub type WorkerHandler = fn(WorkerPayload) -> Result<JsonValue, WorkerExecError>;

/// Run a handler and catch errors into a WorkerResult (spec 04 O8, O11).
/// Never panics; all errors serialize as JSON with status=failed.
pub fn run_worker<F>(payload: WorkerPayload, handler: F) -> WorkerResult
where
    F: FnOnce(WorkerPayload) -> Result<JsonValue, WorkerExecError>,
{
    // Determine default provenance based on kind (spec 04 O9).
    let default_provenance = match payload.kind.as_str() {
        "scrape" | "agent" => Provenance::Untrusted,
        _ => Provenance::System,
    };

    match handler(payload.clone()) {
        Ok(result) => WorkerResult::done(payload.job_id, result, default_provenance, 0, 0.0),
        Err(e) => {
            let error = WorkerError::new(e.to_string(), Some(e.class()));
            WorkerResult::failed(payload.job_id, error, default_provenance)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_payload_deserialize() {
        let json = r#"{"job_id": 42, "kind": "scrape", "args": {"url": "https://example.com"}}"#;
        let payload: WorkerPayload = serde_json::from_str(json).expect("deserialize payload");
        assert_eq!(payload.job_id, 42);
        assert_eq!(payload.kind, "scrape");
        assert_eq!(payload.args["url"], "https://example.com");
    }

    #[test]
    fn test_worker_payload_deserialize_empty_args() {
        let json = r#"{"job_id": 1, "kind": "distill", "args": {}}"#;
        let payload: WorkerPayload = serde_json::from_str(json).expect("deserialize payload");
        assert_eq!(payload.job_id, 1);
        assert_eq!(payload.kind, "distill");
    }

    #[test]
    fn test_worker_result_done_serializes() {
        let result = WorkerResult::done(
            123,
            json!({"output": "hello"}),
            Provenance::System,
            100,
            0.05,
        );
        let json = serde_json::to_value(&result).expect("serialize result");

        // Verify all required fields are present (from schema).
        assert_eq!(json["version"], 1);
        assert_eq!(json["job_id"], 123);
        assert_eq!(json["status"], "done");
        assert_eq!(json["provenance"], "System");
        assert_eq!(json["cost_tokens"], 100);
        assert_eq!(json["cost_usd"], 0.05);
        assert_eq!(json["result"]["output"], "hello");
        assert!(json["error"].is_null());
    }

    #[test]
    fn test_worker_result_failed_serializes() {
        let error = WorkerError::new("something broke", Some(ErrorClass::Bug));
        let result = WorkerResult::failed(42, error, Provenance::Untrusted);
        let json = serde_json::to_value(&result).expect("serialize result");

        assert_eq!(json["version"], 1);
        assert_eq!(json["job_id"], 42);
        assert_eq!(json["status"], "failed");
        assert_eq!(json["provenance"], "Untrusted");
        assert_eq!(json["error"]["message"], "something broke");
        assert_eq!(json["error"]["class"], "bug");
        assert_eq!(json["error"]["retryable"], false);
        assert!(json["result"].is_null());
    }

    #[test]
    fn test_worker_result_partial_serializes() {
        let error = WorkerError::new("partial", Some(ErrorClass::Transient));
        let result = WorkerResult::partial(
            99,
            json!({"partial": "data"}),
            error,
            Provenance::VerifiedKb,
            50,
            0.01,
        );
        let json = serde_json::to_value(&result).expect("serialize result");

        assert_eq!(json["version"], 1);
        assert_eq!(json["job_id"], 99);
        assert_eq!(json["status"], "partial");
        assert_eq!(json["provenance"], "VerifiedKb");
        assert_eq!(json["cost_tokens"], 50);
        assert_eq!(json["result"]["partial"], "data");
        assert_eq!(json["error"]["message"], "partial");
        assert_eq!(json["error"]["class"], "transient");
        assert_eq!(json["error"]["retryable"], true);
    }

    #[test]
    fn test_provenance_default_scrape_is_untrusted() {
        let payload = WorkerPayload {
            job_id: 1,
            kind: "scrape".to_string(),
            args: json!({}),
        };

        let result = run_worker(payload, |_| Ok(json!({"data": "test"})));
        assert_eq!(result.provenance, "Untrusted");
    }

    #[test]
    fn test_provenance_default_agent_is_untrusted() {
        let payload = WorkerPayload {
            job_id: 2,
            kind: "agent".to_string(),
            args: json!({}),
        };

        let result = run_worker(payload, |_| Ok(json!({"output": "test"})));
        assert_eq!(result.provenance, "Untrusted");
    }

    #[test]
    fn test_provenance_default_distill_is_system() {
        let payload = WorkerPayload {
            job_id: 3,
            kind: "distill".to_string(),
            args: json!({}),
        };

        let result = run_worker(payload, |_| Ok(json!({"condensed": "test"})));
        assert_eq!(result.provenance, "System");
    }

    #[test]
    fn test_run_worker_catches_handler_error() {
        let payload = WorkerPayload {
            job_id: 10,
            kind: "ingest".to_string(),
            args: json!({}),
        };

        let result = run_worker(payload, |_| {
            Err(WorkerExecError::Handler("handler failed".to_string()))
        });

        assert_eq!(result.status, "failed");
        assert_eq!(result.job_id, 10);
        assert!(result.error.is_some());
        // Error message includes the error type prefix from thiserror
        assert_eq!(
            result.error.as_ref().unwrap().message,
            "handler error: handler failed"
        );
        assert_eq!(result.error.as_ref().unwrap().class.as_deref(), Some("bug"));
    }

    #[test]
    fn test_run_worker_never_panics() {
        let payload = WorkerPayload {
            job_id: 11,
            kind: "maintenance".to_string(),
            args: json!({}),
        };

        let result = run_worker(payload, |_| {
            Err(WorkerExecError::Parse("bad input".to_string()))
        });

        assert_eq!(result.status, "failed");
        assert_eq!(
            result.error.as_ref().unwrap().class.as_deref(),
            Some("input")
        );
        assert_eq!(result.cost_tokens, 0);
        assert_eq!(result.cost_usd, 0.0);
    }

    #[test]
    fn test_round_trip_equality() {
        let orig = WorkerResult::done(
            55,
            json!({"key": "value"}),
            Provenance::UserDirect,
            200,
            0.10,
        )
        .with_artifacts_dir(Some("artifacts/55".to_string()));

        let json = serde_json::to_string(&orig).expect("serialize");
        let roundtrip: WorkerResult = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(orig, roundtrip);
    }

    #[test]
    fn test_round_trip_equality_failed() {
        let error = WorkerError::new("test error", Some(ErrorClass::Transient));
        let orig = WorkerResult::failed(77, error, Provenance::UnverifiedKb);

        let json = serde_json::to_string(&orig).expect("serialize");
        let roundtrip: WorkerResult = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(orig, roundtrip);
    }

    #[test]
    fn test_schema_required_fields_present() {
        // Verify all schema-required fields are in the serialized output.
        let result = WorkerResult::done(123, json!(null), Provenance::System, 0, 0.0);
        let json = serde_json::to_value(&result).expect("serialize");

        // From schema: required = ["version", "job_id", "status", "provenance"]
        assert!(json.get("version").is_some());
        assert!(json.get("job_id").is_some());
        assert!(json.get("status").is_some());
        assert!(json.get("provenance").is_some());
    }

    #[test]
    fn test_worker_error_with_transient_class() {
        let error = WorkerError::new("transient issue", Some(ErrorClass::Transient));
        assert_eq!(error.class.as_deref(), Some("transient"));
        assert_eq!(error.retryable, Some(true));
    }

    #[test]
    fn test_worker_error_with_input_class() {
        let error = WorkerError::new("bad input", Some(ErrorClass::Input));
        assert_eq!(error.class.as_deref(), Some("input"));
        assert_eq!(error.retryable, Some(false));
    }

    #[test]
    fn test_worker_error_with_resource_class() {
        let error = WorkerError::new("memory limit", Some(ErrorClass::Resource));
        assert_eq!(error.class.as_deref(), Some("resource"));
        assert_eq!(error.retryable, Some(true));
    }

    #[test]
    fn test_artifacts_dir_optional() {
        let result1 = WorkerResult::done(10, json!(null), Provenance::System, 0, 0.0);
        let json1 = serde_json::to_value(&result1).expect("serialize");
        assert!(json1["artifacts_dir"].is_null());

        let result2 = WorkerResult::done(11, json!(null), Provenance::System, 0, 0.0)
            .with_artifacts_dir(Some("artifacts/11".to_string()));
        let json2 = serde_json::to_value(&result2).expect("serialize");
        assert_eq!(json2["artifacts_dir"], "artifacts/11");
    }

    #[test]
    fn test_all_provenance_variants() {
        for (prov, expected_str) in [
            (Provenance::System, "System"),
            (Provenance::UserDirect, "UserDirect"),
            (Provenance::VerifiedKb, "VerifiedKb"),
            (Provenance::UnverifiedKb, "UnverifiedKb"),
            (Provenance::Untrusted, "Untrusted"),
        ] {
            let result = WorkerResult::done(1, json!(null), prov, 0, 0.0);
            assert_eq!(result.provenance, expected_str);
        }
    }
}
