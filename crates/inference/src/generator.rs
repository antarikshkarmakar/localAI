//! Generation execution path (spec 03 I1, I2, I14).
//!
//! `Generator` drives a `GenerationRequest` through the single-flight
//! `InferenceQueue` and the `LlamaTransport`:
//! - waits for the request's turn (priority order, one in flight — I1),
//! - dispatches the full-context request to the transport (I2),
//! - wraps the call in `tokio::time::timeout` (I14): on timeout the caller
//!   gets `InferenceError::Timeout`, the slot is released, and the next
//!   queued request proceeds — a stuck generation never deadlocks the
//!   queue (T8).

use crate::error::InferenceError;
use crate::queue::{GenerationRequest, GenerationResult, InferenceQueue};
use crate::transport::LlamaTransport;
use serde_json::json;
use std::sync::Arc;
use tokio::time::{timeout, Duration};

/// Spec 03: executes generation requests through the single-flight queue.
pub struct Generator {
    queue: Arc<InferenceQueue>,
    transport: Arc<dyn LlamaTransport>,
}

impl Generator {
    /// Create a generator over a shared queue and transport.
    pub fn new(queue: Arc<InferenceQueue>, transport: Arc<dyn LlamaTransport>) -> Self {
        Self { queue, transport }
    }

    /// Submit a generation request and wait for its result.
    ///
    /// Spec 03 I1: strictly one generation in flight; waiting requests are
    /// served in priority order (FIFO within a priority class).
    /// Spec 03 I14: the transport call is bounded by `timeout_ms`; on
    /// timeout this caller gets `InferenceError::Timeout` and the queue
    /// proceeds to the next request (T8 — no deadlock).
    pub async fn generate(
        &self,
        request: GenerationRequest,
        timeout_ms: u64,
    ) -> Result<GenerationResult, InferenceError> {
        let id = self.queue.enqueue(request).await;
        let request = self.queue.wait_for_turn(id).await.ok_or_else(|| {
            InferenceError::Internal("queued request vanished before dispatch".to_string())
        })?;

        // Spec 03 I2: full context in every call — no server-session state.
        let body = json!({
            "prompt": request.prompt,
            "temperature": request.temperature,
            "n_predict": request.max_tokens,
            "seed": request.seed,
        });

        let outcome = timeout(
            Duration::from_millis(timeout_ms),
            self.transport.post_json("/completion", body),
        )
        .await;

        // Release the slot FIRST, on every path — a stuck generation must
        // never hang the queue (I14 protects I1).
        self.queue.mark_complete().await;

        match outcome {
            Err(_elapsed) => Err(InferenceError::Timeout { timeout_ms }),
            Ok(Err(e)) => Err(e),
            Ok(Ok(response)) => {
                let text = response
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        InferenceError::Internal(
                            "generation response missing 'content'".to_string(),
                        )
                    })?;
                let tokens_generated = response
                    .get("tokens_predicted")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                Ok(GenerationResult {
                    text,
                    tokens_generated,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::Priority;
    use serde_json::Value;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::Semaphore;

    fn req(priority: Priority, prompt: &str) -> GenerationRequest {
        GenerationRequest {
            priority,
            prompt: prompt.to_string(),
            temperature: 0.0,
            max_tokens: 16,
            seed: Some(42),
        }
    }

    /// Scripted transport for generator tests:
    /// - logs "start:<prompt>" / "end:<prompt>" events (overlap detection),
    /// - prompt "hang" never completes (pending future) — simulates a stuck
    ///   generation for T8,
    /// - prompts listed in `gated` must acquire a semaphore permit before
    ///   completing — lets a test hold a generation in flight deliberately.
    struct ScriptedTransport {
        events: Arc<StdMutex<Vec<String>>>,
        gate: Arc<Semaphore>,
        gated_prompt: Option<String>,
    }

    impl ScriptedTransport {
        fn new() -> Self {
            Self {
                events: Arc::new(StdMutex::new(Vec::new())),
                gate: Arc::new(Semaphore::new(0)),
                gated_prompt: None,
            }
        }

        fn with_gated_prompt(prompt: &str) -> Self {
            Self {
                gated_prompt: Some(prompt.to_string()),
                ..Self::new()
            }
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl LlamaTransport for ScriptedTransport {
        fn post_json(
            &self,
            _path: &str,
            body: Value,
        ) -> Pin<Box<dyn Future<Output = Result<Value, InferenceError>> + Send + '_>> {
            let events = Arc::clone(&self.events);
            let gate = Arc::clone(&self.gate);
            let gated = self.gated_prompt.clone();
            Box::pin(async move {
                let prompt = body
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                events.lock().unwrap().push(format!("start:{prompt}"));

                if prompt == "hang" {
                    // Never completes — the per-request timeout must fire.
                    std::future::pending::<()>().await;
                }
                if gated.as_deref() == Some(prompt.as_str()) {
                    let permit = gate.acquire().await.unwrap();
                    permit.forget();
                }
                // Small delay so overlapping dispatches would interleave.
                tokio::time::sleep(Duration::from_millis(10)).await;

                events.lock().unwrap().push(format!("end:{prompt}"));
                Ok(json!({
                    "content": format!("echo:{prompt}"),
                    "tokens_predicted": 1
                }))
            })
        }
    }

    // Spec 03 T8 / I14: a hung generation times out with an error, and the
    // next queued request still executes — the queue never deadlocks.
    #[tokio::test]
    async fn timeout_unblocks_queue() {
        let transport = Arc::new(ScriptedTransport::new());
        let queue = Arc::new(InferenceQueue::new());
        let generator = Generator::new(Arc::clone(&queue), transport.clone());

        let hung = generator
            .generate(req(Priority::Interactive, "hang"), 50)
            .await;
        assert!(
            matches!(hung, Err(InferenceError::Timeout { timeout_ms: 50 })),
            "hung generation must return Timeout, got {hung:?}"
        );

        // Queue must have released the slot — the next request runs fine.
        let next = generator
            .generate(req(Priority::Background, "next"), 1_000)
            .await
            .unwrap();
        assert_eq!(next.text, "echo:next");
        assert!(!queue.is_generation_in_flight().await);
    }

    // Spec 03 I1 through the FULL path: 3 concurrent submissions reach the
    // transport strictly sequentially — every start is followed by its own
    // end before the next start.
    #[tokio::test]
    async fn concurrent_submissions_are_strictly_sequential() {
        let transport = Arc::new(ScriptedTransport::new());
        let queue = Arc::new(InferenceQueue::new());
        let generator = Arc::new(Generator::new(queue, transport.clone()));

        let (r1, r2, r3) = tokio::join!(
            generator.generate(req(Priority::Interactive, "a"), 5_000),
            generator.generate(req(Priority::SelfCheck, "b"), 5_000),
            generator.generate(req(Priority::Background, "c"), 5_000),
        );
        assert!(r1.is_ok() && r2.is_ok() && r3.is_ok());

        let events = transport.events();
        assert_eq!(events.len(), 6, "3 start + 3 end events: {events:?}");
        for pair in events.chunks(2) {
            let start = pair[0].strip_prefix("start:");
            let end = pair[1].strip_prefix("end:");
            assert!(
                start.is_some() && start == end,
                "generations overlapped: {events:?}"
            );
        }
    }

    // Priority through the full path: while a generation is in flight,
    // queue Background then Interactive; Interactive runs first when the
    // slot frees.
    #[tokio::test]
    async fn interactive_preempts_waiting_background() {
        let transport = Arc::new(ScriptedTransport::with_gated_prompt("first"));
        let queue = Arc::new(InferenceQueue::new());
        let generator = Arc::new(Generator::new(Arc::clone(&queue), transport.clone()));

        // Hold "first" in flight (blocked on the gate inside the transport).
        let g = Arc::clone(&generator);
        let t_first = tokio::spawn(async move {
            g.generate(req(Priority::Interactive, "first"), 60_000)
                .await
        });
        while !queue.is_generation_in_flight().await {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Queue Background first, then Interactive.
        let g = Arc::clone(&generator);
        let t_bg =
            tokio::spawn(async move { g.generate(req(Priority::Background, "bg"), 60_000).await });
        while queue.pending_count().await < 1 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let g = Arc::clone(&generator);
        let t_int =
            tokio::spawn(
                async move { g.generate(req(Priority::Interactive, "int"), 60_000).await },
            );
        while queue.pending_count().await < 2 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }

        // Release the in-flight generation.
        transport.gate.add_permits(1);

        let (r_first, r_bg, r_int) = tokio::join!(t_first, t_bg, t_int);
        assert!(r_first.unwrap().is_ok());
        assert!(r_bg.unwrap().is_ok());
        assert!(r_int.unwrap().is_ok());

        let starts: Vec<String> = transport
            .events()
            .into_iter()
            .filter(|e| e.starts_with("start:"))
            .collect();
        assert_eq!(
            starts,
            vec!["start:first", "start:int", "start:bg"],
            "Interactive must run before waiting Background"
        );
    }

    // Transport error (non-timeout) also releases the slot.
    #[tokio::test]
    async fn transport_error_releases_slot() {
        struct FailingTransport;
        impl LlamaTransport for FailingTransport {
            fn post_json(
                &self,
                _path: &str,
                _body: Value,
            ) -> Pin<Box<dyn Future<Output = Result<Value, InferenceError>> + Send + '_>>
            {
                Box::pin(async {
                    Err(InferenceError::ServerError {
                        status: 500,
                        message: "boom".to_string(),
                    })
                })
            }
        }

        let queue = Arc::new(InferenceQueue::new());
        let generator = Generator::new(Arc::clone(&queue), Arc::new(FailingTransport));

        let result = generator
            .generate(req(Priority::Interactive, "x"), 1_000)
            .await;
        assert!(matches!(result, Err(InferenceError::ServerError { .. })));
        assert!(!queue.is_generation_in_flight().await);
    }

    // Successful generation parses content + token count.
    #[tokio::test]
    async fn generate_parses_result() {
        let transport = Arc::new(ScriptedTransport::new());
        let queue = Arc::new(InferenceQueue::new());
        let generator = Generator::new(queue, transport);

        let result = generator
            .generate(req(Priority::Interactive, "hello"), 1_000)
            .await
            .unwrap();
        assert_eq!(result.text, "echo:hello");
        assert_eq!(result.tokens_generated, 1);
    }
}
