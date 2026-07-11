//! HTTP transport abstraction (spec 03, standards.md trait testability).
//!
//! `trait LlamaTransport` abstracts HTTP POST to llama-server;
//! `HttpTransport` is the reqwest-based impl; tests provide mocks.

use crate::error::InferenceError;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

/// Spec 03: HTTP abstraction for llama-server calls.
///
/// Enables mocking in tests and hot-swap of transport backends.
pub trait LlamaTransport: Send + Sync {
    /// POST JSON to a path, return JSON response.
    /// Spec 03: All server calls use this (generation, tokenize, health, embeddings).
    fn post_json(
        &self,
        path: &str,
        body: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, InferenceError>> + Send + '_>>;
}

/// Spec 03: reqwest-based HTTP transport for llama-server on loopback.
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
}

impl HttpTransport {
    /// Create a new HTTP transport to the given loopback URL.
    /// E.g.: `http://127.0.0.1:8080`
    pub fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }
}

impl LlamaTransport for HttpTransport {
    fn post_json(
        &self,
        path: &str,
        body: Value,
    ) -> Pin<Box<dyn Future<Output = Result<Value, InferenceError>> + Send + '_>> {
        let base_url = self.base_url.clone();
        let client = self.client.clone();
        let path = path.to_string();
        Box::pin(async move {
            let url = format!("{}{}", base_url, path);

            let response = client.post(&url).json(&body).send().await?;

            if !response.status().is_success() {
                let status = response.status().as_u16();
                let text = response.text().await.unwrap_or_default();
                return Err(InferenceError::ServerError {
                    status,
                    message: text,
                });
            }

            let resp_json = response.json::<Value>().await?;
            Ok(resp_json)
        })
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};

    /// Test mock that records calls and replays responses.
    pub struct MockTransport {
        responses: Arc<StdMutex<Vec<Value>>>,
        calls: Arc<StdMutex<Vec<(String, Value)>>>,
    }

    impl MockTransport {
        pub fn new() -> Self {
            Self {
                responses: Arc::new(StdMutex::new(Vec::new())),
                calls: Arc::new(StdMutex::new(Vec::new())),
            }
        }

        pub fn enqueue_response(&self, resp: Value) {
            self.responses.lock().unwrap().push(resp);
        }

        pub fn recorded_calls(&self) -> Vec<(String, Value)> {
            self.calls.lock().unwrap().clone()
        }

        pub fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl Default for MockTransport {
        fn default() -> Self {
            Self::new()
        }
    }

    impl LlamaTransport for MockTransport {
        fn post_json(
            &self,
            path: &str,
            body: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, InferenceError>> + Send + '_>> {
            let path_str = path.to_string();
            let body_clone = body.clone();
            Box::pin(async move {
                self.calls.lock().unwrap().push((path_str, body_clone));

                let mut resps = self.responses.lock().unwrap();
                if resps.is_empty() {
                    return Err(InferenceError::Internal(
                        "MockTransport: no responses enqueued".to_string(),
                    ));
                }
                Ok(resps.remove(0))
            })
        }
    }

    #[tokio::test]
    async fn http_transport_constructs() {
        let transport = HttpTransport::new("http://127.0.0.1:8080".to_string());
        assert_eq!(transport.base_url, "http://127.0.0.1:8080");
    }

    #[tokio::test]
    async fn mock_transport_enqueues_responses() {
        let mock = MockTransport::new();
        mock.enqueue_response(json!({"result": "ok"}));

        let resp = mock
            .post_json("/test", json!({"input": "data"}))
            .await
            .unwrap();

        assert_eq!(resp, json!({"result": "ok"}));
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test]
    async fn mock_transport_records_calls() {
        let mock = MockTransport::new();
        mock.enqueue_response(json!({}));

        let body = json!({"prompt": "hello"});
        mock.post_json("/completion", body.clone()).await.ok();

        let calls = mock.recorded_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "/completion");
        assert_eq!(calls[0].1, body);
    }
}
