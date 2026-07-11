//! Health check for llama-server (spec 03 I13).
//!
//! Polls /health endpoint; just the call + typed result.
//! Restart policy is supervisor's job (spec 09).

use crate::error::InferenceError;
use crate::transport::LlamaTransport;
use serde_json::json;
use std::sync::Arc;

/// Spec 03 I13: Health check result.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthStatus {
    pub status: String,
    pub model: Option<String>,
}

/// Spec 03 I13: Health check caller.
pub struct HealthCheck {
    transport: Arc<dyn LlamaTransport>,
}

impl HealthCheck {
    pub fn new(transport: Arc<dyn LlamaTransport>) -> Self {
        Self { transport }
    }

    /// Poll /health endpoint, return typed result.
    pub async fn check(&self) -> Result<HealthStatus, InferenceError> {
        let response = self.transport.post_json("/health", json!({})).await?;

        let status = response
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                InferenceError::Internal("health response missing 'status'".to_string())
            })?;

        let model = response
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Ok(HealthStatus { status, model })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::tests::MockTransport;

    #[tokio::test]
    async fn health_check_parses_status() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"status": "ok", "model": "gemma4-12b"}));

        let health = HealthCheck::new(mock);
        let result = health.check().await.unwrap();

        assert_eq!(result.status, "ok");
        assert_eq!(result.model, Some("gemma4-12b".to_string()));
    }

    #[tokio::test]
    async fn health_check_handles_missing_model() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"status": "loading"}));

        let health = HealthCheck::new(mock);
        let result = health.check().await.unwrap();

        assert_eq!(result.status, "loading");
        assert_eq!(result.model, None);
    }

    #[tokio::test]
    async fn health_check_errors_on_missing_status() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"model": "gemma4-12b"}));

        let health = HealthCheck::new(mock);
        let result = health.check().await;

        assert!(result.is_err());
    }
}
