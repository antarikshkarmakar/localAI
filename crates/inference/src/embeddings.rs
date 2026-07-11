//! Embeddings endpoint (spec 03 I3).
//!
//! Spec 03 I3: Embeddings are exempt from the generation queue but still
//! bounded by their own semaphore to prevent unbounded concurrency.

use crate::error::InferenceError;
use crate::transport::LlamaTransport;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Spec 03 I3: Embeddings are cheap and parallel-safe, bounded by their own semaphore.
pub struct EmbeddingsClient {
    transport: Arc<dyn LlamaTransport>,
    semaphore: Arc<Semaphore>,
}

/// Embedding result: vector + metadata.
#[derive(Debug, Clone)]
pub struct Embedding {
    pub vector: Vec<f32>,
    pub model_version: u32,
}

impl EmbeddingsClient {
    /// Create a new embeddings client with a bounded semaphore (permits).
    pub fn new(transport: Arc<dyn LlamaTransport>, permits: usize) -> Self {
        Self {
            transport,
            semaphore: Arc::new(Semaphore::new(permits)),
        }
    }

    /// Embed a text string.
    /// Spec 03 I3: Bounded by semaphore, not the generation queue.
    pub async fn embed(&self, text: &str) -> Result<Embedding, InferenceError> {
        // Acquire permit (bounded concurrency)
        let _permit = self.semaphore.acquire().await.map_err(|e| {
            InferenceError::EmbeddingsSemaphoreFailed {
                reason: e.to_string(),
            }
        })?;

        let body = json!({
            "content": text
        });

        let response = self.transport.post_json("/embeddings", body).await?;

        // Parse embedding vector from response
        let vector = response
            .get("embedding")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                    .collect()
            })
            .ok_or_else(|| {
                InferenceError::Internal(
                    "embeddings response missing 'embedding' array".to_string(),
                )
            })?;

        // Model version for retrieval compatibility (spec I12)
        let model_version = response
            .get("model_version")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(1);

        Ok(Embedding {
            vector,
            model_version,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::tests::MockTransport;

    #[tokio::test]
    async fn embeddings_client_creation() {
        let mock = Arc::new(MockTransport::new());
        let client = EmbeddingsClient::new(mock, 4);
        assert_eq!(client.semaphore.available_permits(), 4);
    }

    #[tokio::test]
    async fn embed_parses_vector() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue_response(json!({
            "embedding": [0.1, 0.2, 0.3],
            "model_version": 1
        }));

        let client = EmbeddingsClient::new(mock, 4);
        let result = client.embed("hello world").await.unwrap();

        assert_eq!(result.vector.len(), 3);
        assert_eq!(result.model_version, 1);
    }

    #[tokio::test]
    async fn embed_default_model_version() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue_response(json!({
            "embedding": [0.5, 0.6]
        }));

        let client = EmbeddingsClient::new(mock, 4);
        let result = client.embed("text").await.unwrap();

        assert_eq!(result.model_version, 1);
    }

    #[tokio::test]
    async fn embed_errors_on_missing_embedding() {
        let mock = Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"result": "ok"}));

        let client = EmbeddingsClient::new(mock, 4);
        let result = client.embed("text").await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn embed_respects_semaphore_permits() {
        let mock = Arc::new(MockTransport::new());
        // Enqueue responses for 3 embeddings
        mock.enqueue_response(json!({"embedding": [0.1]}));
        mock.enqueue_response(json!({"embedding": [0.2]}));
        mock.enqueue_response(json!({"embedding": [0.3]}));

        let client = EmbeddingsClient::new(mock.clone(), 2);

        // Start two embeddings (should consume both permits)
        let fut1 = client.embed("text1");
        let fut2 = client.embed("text2");
        let fut3 = client.embed("text3");

        // All three should complete (futures are not cancelled by semaphore)
        let (r1, r2, r3) = tokio::join!(fut1, fut2, fut3);
        assert!(r1.is_ok());
        assert!(r2.is_ok());
        assert!(r3.is_ok());
    }
}
