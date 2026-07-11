//! Tokenizer with content-hash cache (spec 03 I6).
//!
//! Token counts come from model's /tokenize endpoint; cache by string hash
//! to avoid redundant tokenizer calls.

use crate::error::InferenceError;
use crate::transport::LlamaTransport;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;

/// Spec 03 I6: Tokenize cache keyed by content hash.
pub struct TokenizeCache {
    cache: Mutex<HashMap<String, u32>>,
    transport: std::sync::Arc<dyn LlamaTransport>,
}

impl TokenizeCache {
    /// Create a new tokenize cache backed by the given transport.
    pub fn new(transport: std::sync::Arc<dyn LlamaTransport>) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            transport,
        }
    }

    /// Tokenize a text string, using cache on second+ calls.
    /// Spec 03 I6: cached by string hash.
    pub async fn tokenize(&self, text: &str) -> Result<u32, InferenceError> {
        let hash = Self::content_hash(text);

        // Check cache first
        {
            let cache = self
                .cache
                .lock()
                .map_err(|e| InferenceError::Internal(format!("tokenize cache poisoned: {}", e)))?;
            if let Some(&count) = cache.get(&hash) {
                return Ok(count);
            }
        }

        // Cache miss: call /tokenize endpoint
        let body = json!({
            "content": text
        });

        let response = self.transport.post_json("/tokenize", body).await?;

        // Parse token count from response
        // Spec 03: response format TBD, assume { "tokens": [...] } or { "token_count": N }
        let count = response
            .get("tokens")
            .and_then(|v| v.as_array())
            .map(|arr| arr.len() as u32)
            .or_else(|| {
                response
                    .get("token_count")
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32)
            })
            .ok_or_else(|| {
                InferenceError::Internal(
                    "tokenize response missing 'tokens' or 'token_count'".to_string(),
                )
            })?;

        // Cache the result
        self.cache
            .lock()
            .map_err(|e| InferenceError::Internal(format!("tokenize cache poisoned: {}", e)))?
            .insert(hash, count);

        Ok(count)
    }

    fn content_hash(text: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::tests::MockTransport;

    #[tokio::test]
    async fn tokenize_hits_transport_on_first_call() {
        let mock = std::sync::Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"tokens": ["hello", "world"]}));

        let cache = TokenizeCache::new(mock.clone());
        let count = cache.tokenize("hello world").await.unwrap();

        assert_eq!(count, 2);
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test]
    async fn tokenize_cache_prevents_second_call() {
        let mock = std::sync::Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"tokens": ["hello", "world"]}));

        let cache = TokenizeCache::new(mock.clone());
        let count1 = cache.tokenize("hello world").await.unwrap();
        let count2 = cache.tokenize("hello world").await.unwrap();

        assert_eq!(count1, 2);
        assert_eq!(count2, 2);
        // Only one transport call — second was cached
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test]
    async fn tokenize_cache_distinguishes_different_texts() {
        let mock = std::sync::Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"tokens": ["hello"]}));
        mock.enqueue_response(json!({"tokens": ["goodbye", "world"]}));

        let cache = TokenizeCache::new(mock.clone());
        let count1 = cache.tokenize("hello").await.unwrap();
        let count2 = cache.tokenize("goodbye world").await.unwrap();

        assert_eq!(count1, 1);
        assert_eq!(count2, 2);
        assert_eq!(mock.call_count(), 2);
    }

    #[tokio::test]
    async fn tokenize_parses_token_count_field() {
        let mock = std::sync::Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"token_count": 42}));

        let cache = TokenizeCache::new(mock.clone());
        let count = cache.tokenize("some text").await.unwrap();

        assert_eq!(count, 42);
    }

    #[tokio::test]
    async fn tokenize_errors_on_missing_response_field() {
        let mock = std::sync::Arc::new(MockTransport::new());
        mock.enqueue_response(json!({"result": "ok"}));

        let cache = TokenizeCache::new(mock.clone());
        let result = cache.tokenize("text").await;

        assert!(result.is_err());
    }
}
