//! Inference client (spec 03).
//!
//! Provides:
//! - `trait LlamaTransport` — HTTP abstraction
//! - `InferenceQueue` — single-generation queue with 3 priority classes (I1)
//! - `tokenize()` with content-hash cache (I6)
//! - `health()` check (I13)
//! - Embeddings endpoint with own semaphore (I3)
//! - Per-request timeout (I14) — no queue deadlocks

pub mod embeddings;
pub mod error;
pub mod generator;
pub mod health;
pub mod launch;
pub mod queue;
pub mod tokenize;
pub mod transport;

pub use embeddings::{Embedding, EmbeddingsClient};
pub use error::InferenceError;
pub use generator::Generator;
pub use health::HealthCheck;
pub use queue::{GenerationRequest, GenerationResult, InferenceQueue, Priority};
pub use tokenize::TokenizeCache;
pub use transport::{HttpTransport, LlamaTransport};
