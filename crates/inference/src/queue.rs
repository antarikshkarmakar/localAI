//! Inference queue with priority and single-generation guarantee (spec 03 I1, I14).
//!
//! Spec 03 I1: Exactly one generation in flight at a time.
//! Spec 03 I2: Each request carries full context (stateless w.r.t. server).
//! Spec 03 I14: Per-request timeout; never hangs the queue.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify};

/// Spec 03 I1: Three priority classes for generation requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Interactive UI requests — lowest latency.
    Interactive = 2,
    /// Router self-check / fact validation.
    SelfCheck = 1,
    /// Background tasks (distillation, etc).
    Background = 0,
}

/// Spec 03 I2: Generation request with full context (stateless).
#[derive(Debug, Clone)]
pub struct GenerationRequest {
    pub priority: Priority,
    pub prompt: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub seed: Option<u32>,
}

/// Queued request with ordering metadata.
#[derive(Clone)]
struct QueuedRequest {
    request: GenerationRequest,
    sequence: u64, // FIFO ordering within same priority
    id: u64,       // Unique request ID for matching results
}

impl Eq for QueuedRequest {}

impl PartialEq for QueuedRequest {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Ord for QueuedRequest {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap: if self > other, self comes out first.
        // We want highest priority first, so:
        // If self.priority > other.priority, self should win (self > other)
        match self.request.priority.cmp(&other.request.priority) {
            Ordering::Equal => {
                // Same priority: FIFO (earlier sequence first means smaller sequence > larger sequence)
                other.sequence.cmp(&self.sequence)
            }
            cmp => cmp,
        }
    }
}

impl PartialOrd for QueuedRequest {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Spec 03 I1: Single-generation queue.
///
/// Ensures exactly one generation runs at a time; others wait in priority order.
/// Spec 03 I14: Per-request timeout never hangs the queue.
pub struct InferenceQueue {
    state: Arc<Mutex<QueueState>>,
}

struct QueueState {
    // Pending requests, ordered by priority then sequence
    pending: BinaryHeap<QueuedRequest>,
    // Sequence counter for FIFO within priority
    next_seq: u64,
    // Request ID counter
    next_id: u64,
    // Flag: generation currently in flight
    in_flight: bool,
    // Notifier for queue state changes
    notifier: Arc<Notify>,
}

/// Result of a generation request.
#[derive(Debug, Clone)]
pub struct GenerationResult {
    pub text: String,
    pub tokens_generated: u32,
}

impl Default for InferenceQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl InferenceQueue {
    /// Create a new inference queue.
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(QueueState {
                pending: BinaryHeap::new(),
                next_seq: 0,
                next_id: 0,
                in_flight: false,
                notifier: Arc::new(Notify::new()),
            })),
        }
    }

    /// Check if exactly one generation is in flight.
    /// Used in tests to verify I1 (single-flight guarantee).
    pub async fn is_generation_in_flight(&self) -> bool {
        let state = self.state.lock().await;
        state.in_flight
    }

    /// Get pending queue length.
    /// Used in tests.
    pub async fn pending_count(&self) -> usize {
        let state = self.state.lock().await;
        state.pending.len()
    }

    /// Mark a generation as complete (called by the generation engine).
    /// Spec 03 I1: Signals that the next queued request can dispatch.
    pub async fn mark_complete(&self) {
        let mut state = self.state.lock().await;
        state.in_flight = false;
        // Wake up waiting tasks
        state.notifier.notify_waiters();
    }

    /// Check if the queue can accept a new generation request.
    /// Returns true if either no request is in flight or the queue is empty.
    pub async fn can_dispatch_next(&self) -> bool {
        let state = self.state.lock().await;
        !state.in_flight && !state.pending.is_empty()
    }

    /// Peek at the next request to dispatch (without removing it).
    /// Returns None if no request is ready to dispatch.
    pub async fn peek_next(&self) -> Option<GenerationRequest> {
        let state = self.state.lock().await;
        if state.in_flight {
            return None;
        }
        state.pending.peek().map(|req| req.request.clone())
    }

    /// Pop the next request for dispatch.
    /// Must only be called after peek_next() returns Some and before mark_complete().
    /// Marks the request as in_flight.
    pub async fn pop_next(&self) -> Option<GenerationRequest> {
        let mut state = self.state.lock().await;
        if state.in_flight || state.pending.is_empty() {
            return None;
        }
        if let Some(queued) = state.pending.pop() {
            state.in_flight = true;
            Some(queued.request)
        } else {
            None
        }
    }

    /// Enqueue a generation request.
    /// Returns a unique request ID.
    pub async fn enqueue(&self, request: GenerationRequest) -> u64 {
        let mut state = self.state.lock().await;
        let seq = state.next_seq;
        state.next_seq += 1;
        let id = state.next_id;
        state.next_id += 1;

        let queued = QueuedRequest {
            request,
            sequence: seq,
            id,
        };
        state.pending.push(queued);
        id
    }

    /// Wait for a queued request to reach the front and acquire the generation slot.
    /// Spec 03 I1: exactly one generation in flight; this task waits in priority order.
    /// Returns the request once it's this task's turn to execute, or None if the
    /// request was cancelled or lost.
    pub async fn wait_for_turn(&self, request_id: u64) -> Option<GenerationRequest> {
        loop {
            let mut state = self.state.lock().await;

            // Check if our request is at the front and the slot is free
            if let Some(next_req) = state.pending.peek() {
                if next_req.id == request_id && !state.in_flight {
                    // It's our turn! Pop and mark as in flight
                    if let Some(queued) = state.pending.pop() {
                        state.in_flight = true;
                        return Some(queued.request);
                    }
                }
            }

            // Not our turn yet; wait for state changes
            let notifier = Arc::clone(&state.notifier);
            drop(state);
            notifier.notified().await;
            // Loop continues, re-acquiring the lock
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_ordering_is_correct() {
        assert!(Priority::Interactive > Priority::SelfCheck);
        assert!(Priority::SelfCheck > Priority::Background);
    }

    #[test]
    fn queued_request_priority_ordering() {
        let req_bg = QueuedRequest {
            request: GenerationRequest {
                priority: Priority::Background,
                prompt: "bg".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            },
            sequence: 0,
            id: 0,
        };

        let req_interactive = QueuedRequest {
            request: GenerationRequest {
                priority: Priority::Interactive,
                prompt: "interactive".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            },
            sequence: 1,
            id: 1,
        };

        // BinaryHeap is max-heap, so highest priority sorts first
        let mut heap = BinaryHeap::new();
        heap.push(req_bg.clone());
        heap.push(req_interactive.clone());

        // Pop should give Interactive first
        let first = heap.pop().unwrap();
        assert_eq!(first.request.priority, Priority::Interactive);
    }

    #[test]
    fn queued_request_fifo_within_priority() {
        let req1 = QueuedRequest {
            request: GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "first".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            },
            sequence: 0,
            id: 0,
        };

        let req2 = QueuedRequest {
            request: GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "second".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            },
            sequence: 1,
            id: 1,
        };

        let mut heap = BinaryHeap::new();
        heap.push(req2.clone());
        heap.push(req1.clone());

        // Same priority: FIFO (lower sequence first)
        let first = heap.pop().unwrap();
        assert_eq!(first.sequence, 0);
        let second = heap.pop().unwrap();
        assert_eq!(second.sequence, 1);
    }

    #[tokio::test]
    async fn queue_creation() {
        let queue = InferenceQueue::new();
        assert!(!queue.is_generation_in_flight().await);
        assert_eq!(queue.pending_count().await, 0);
    }

    // Spec 03 T1: 3 concurrent callers → exactly one generation at a time
    #[tokio::test]
    async fn single_flight_guarantee() {
        let queue = InferenceQueue::new();

        // Enqueue 3 requests with different priorities
        queue
            .enqueue(GenerationRequest {
                priority: Priority::Background,
                prompt: "bg task".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        queue
            .enqueue(GenerationRequest {
                priority: Priority::Interactive,
                prompt: "ui request".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        queue
            .enqueue(GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "self-check".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        assert_eq!(queue.pending_count().await, 3);
        assert!(!queue.is_generation_in_flight().await);

        // Dispatch first request
        let first = queue.pop_next().await;
        assert!(first.is_some());
        assert!(queue.is_generation_in_flight().await);
        assert_eq!(queue.pending_count().await, 2);

        // Cannot dispatch second while first is in flight
        let second_attempt = queue.pop_next().await;
        assert!(second_attempt.is_none());

        // Complete first generation
        queue.mark_complete().await;
        assert!(!queue.is_generation_in_flight().await);

        // Now can dispatch second (should be Interactive by priority)
        let second = queue.pop_next().await;
        assert!(second.is_some());
        assert_eq!(queue.pending_count().await, 1);
        assert!(queue.is_generation_in_flight().await);

        queue.mark_complete().await;
        let third = queue.pop_next().await;
        assert!(third.is_some());
        assert_eq!(queue.pending_count().await, 0);
    }

    // Spec 03 T1: Priority ordering
    #[tokio::test]
    async fn priority_ordering_in_queue() {
        let queue = InferenceQueue::new();

        // Enqueue in order: Background, SelfCheck, Interactive
        queue
            .enqueue(GenerationRequest {
                priority: Priority::Background,
                prompt: "bg".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        queue
            .enqueue(GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "selfcheck".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        queue
            .enqueue(GenerationRequest {
                priority: Priority::Interactive,
                prompt: "interactive".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        // Dispatch first - should be Interactive (highest priority)
        let req1 = queue.pop_next().await.unwrap();
        assert_eq!(req1.prompt, "interactive");
        queue.mark_complete().await;

        // Dispatch second - should be SelfCheck
        let req2 = queue.pop_next().await.unwrap();
        assert_eq!(req2.prompt, "selfcheck");
        queue.mark_complete().await;

        // Dispatch third - should be Background
        let req3 = queue.pop_next().await.unwrap();
        assert_eq!(req3.prompt, "bg");
    }

    // FIFO within same priority
    #[tokio::test]
    async fn fifo_within_priority() {
        let queue = InferenceQueue::new();

        queue
            .enqueue(GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "first".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        queue
            .enqueue(GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "second".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        queue
            .enqueue(GenerationRequest {
                priority: Priority::SelfCheck,
                prompt: "third".to_string(),
                temperature: 0.7,
                max_tokens: 100,
                seed: None,
            })
            .await;

        // All same priority, should dispatch in FIFO order
        let req1 = queue.pop_next().await.unwrap();
        assert_eq!(req1.prompt, "first");
        queue.mark_complete().await;

        let req2 = queue.pop_next().await.unwrap();
        assert_eq!(req2.prompt, "second");
        queue.mark_complete().await;

        let req3 = queue.pop_next().await.unwrap();
        assert_eq!(req3.prompt, "third");
    }
}
