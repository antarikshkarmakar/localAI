//! Error types for inference operations (spec 03, standards.md).
//!
//! Typed errors via thiserror; each variant maps to the failure category
//! (transient/input/resource/bug) where it crosses job boundaries.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InferenceError {
    // Transient: retry-safe
    #[error("inference request timed out after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },

    #[error("llama-server health check failed: {reason}")]
    HealthCheckFailed { reason: String },

    // Resource: queue/semaphore exhaustion
    #[error("inference queue is at capacity")]
    QueueFull,

    #[error("embeddings semaphore acquire failed: {reason}")]
    EmbeddingsSemaphoreFailed { reason: String },

    // Transport / input
    #[error("HTTP transport error: {0}")]
    TransportError(String),

    #[error("llama-server returned error: {status} {message}")]
    ServerError { status: u16, message: String },

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("context size {requested} exceeds limit {limit}")]
    ContextTooLarge { requested: u32, limit: u32 },

    // Bug
    #[error("internal inference error: {0}")]
    Internal(String),
}

impl From<reqwest::Error> for InferenceError {
    fn from(err: reqwest::Error) -> Self {
        InferenceError::TransportError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_error_message_is_clear() {
        let err = InferenceError::Timeout { timeout_ms: 5000 };
        assert_eq!(err.to_string(), "inference request timed out after 5000ms");
    }

    #[test]
    fn context_too_large_error_shows_values() {
        let err = InferenceError::ContextTooLarge {
            requested: 40000,
            limit: 32768,
        };
        assert_eq!(err.to_string(), "context size 40000 exceeds limit 32768");
    }
}
