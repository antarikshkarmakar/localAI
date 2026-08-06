//! CouncilMember trait and types (spec 05 §2).

use crate::{Cost, CouncilCtx, Prompt, Verdict};
use async_trait::async_trait;
use thiserror::Error;

/// Error from a council member call.
#[derive(Debug, Clone, Error)]
pub enum MemberError {
    #[error("member {member} network error: {details}")]
    Network { member: String, details: String },

    #[error("member {member} timeout")]
    Timeout { member: String },

    #[error("member {member} invalid response: {details}")]
    InvalidResponse { member: String, details: String },

    #[error("member {member} refused: {reason}")]
    Refused { member: String, reason: String },

    #[error("member {member} unavailable")]
    Unavailable { member: String },
}

/// One cloud LLM adapter (spec 05 C1/C2).
///
/// Implementations must:
/// 1. Read API keys from env only (CON-9)
/// 2. Ping model liveness at startup (C2)
/// 3. Have a circuit breaker for consecutive failures (C3)
#[async_trait]
pub trait CouncilMember: Send + Sync {
    /// Provider ID: 'anthropic' | 'openai' | 'gemini'
    fn id(&self) -> &str;

    /// Model ID from config
    fn model(&self) -> &str;

    /// Ask the member for a verdict on a prompt (spec 05 C1).
    /// CALLER RESPONSIBILITY: filter the prompt through localai_security::filter()
    /// before calling this (spec 07 H9, C1). The trait does NOT re-filter.
    async fn ask(&self, prompt: Prompt, ctx: &CouncilCtx) -> Result<Verdict, MemberError>;

    /// Estimate cost for a prompt (spec 05 C14).
    fn cost_estimate(&self, prompt: &Prompt) -> Cost;

    /// Is this member currently available? (C3 circuit breaker)
    fn is_available(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_error_display() {
        let err = MemberError::Network {
            member: "anthropic".to_string(),
            details: "connection timeout".to_string(),
        };
        assert!(format!("{}", err).contains("network error"));
    }
}
