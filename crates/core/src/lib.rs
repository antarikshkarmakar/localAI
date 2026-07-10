//! Core types, error handling, and traits for localAI Brain.
//!
//! Cites: spec 00 (OBJ-*, CON-*, KPI-*), spec 01 (architecture).

pub mod config;
pub mod mem_guard;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Event kinds (spec 01, ledger events).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventKind {
    OnRoute,
    OnTask,
    OnError,
    OnLearning,
    OnEgress,
}

/// Error taxonomy (spec 09 §2).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorClass {
    Transient,
    Input,
    Bug,
    Resource,
}

/// Brain-level error.
#[derive(Debug, Error)]
pub enum BrainError {
    #[error("database error: {0}")]
    Database(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("inference error: {0}")]
    Inference(String),

    #[error("memory guard: {0}")]
    MemoryGuard(String),

    #[error("security violation: {0}")]
    Security(String),
}

/// Provenance tags (spec 07 H3).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Provenance {
    System,
    UserDirect,
    VerifiedKb,
    UnverifiedKb,
    Untrusted,
}

impl Provenance {
    pub fn is_trusted(&self) -> bool {
        matches!(
            self,
            Provenance::System | Provenance::UserDirect | Provenance::VerifiedKb
        )
    }
}

/// Task/job status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Partial,
    Failed,
    Quarantined,
}

/// Route choice (spec 06).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Route {
    Local,
    LocalSelfcheck,
    Search,
    CouncilDecide,
    CouncilFact,
    Agent,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_trust_levels() {
        assert!(Provenance::System.is_trusted());
        assert!(Provenance::VerifiedKb.is_trusted());
        assert!(!Provenance::Untrusted.is_trusted());
        assert!(!Provenance::UnverifiedKb.is_trusted());
    }
}
