//! Real adapter stubs (structure only; no live calls in tests).
//!
//! The actual HTTP implementations should be in brain/server integration,
//! not tested with network calls in this crate.

use crate::{Cost, CouncilCtx, CouncilMember, MemberError, Prompt, Verdict};
use async_trait::async_trait;
use std::env;

/// Thin HTTP adapter for a cloud LLM (spec 05 C1/C2).
///
/// Keys are read from env (CON-9). Actual HTTP calls deferred to integration layer.
pub struct HttpMember {
    id: String,
    model: String,
    #[allow(dead_code)]
    api_key: String,
}

impl HttpMember {
    /// Create an HTTP member from environment (spec 05 C1, CON-9).
    /// Panics at startup if key is missing (C2 — liveness check at boot).
    pub fn from_env(provider_id: &str, model_id: &str) -> Result<Self, String> {
        let env_key = format!("COUNCIL_{}_API_KEY", provider_id.to_uppercase());
        let api_key =
            env::var(&env_key).map_err(|_| format!("missing {} (required by CON-9)", env_key))?;

        Ok(Self {
            id: provider_id.to_string(),
            model: model_id.to_string(),
            api_key,
        })
    }
}

#[async_trait]
impl CouncilMember for HttpMember {
    fn id(&self) -> &str {
        &self.id
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn ask(&self, _prompt: Prompt, _ctx: &CouncilCtx) -> Result<Verdict, MemberError> {
        // Real implementation: deserialize the Prompt, call the provider's API,
        // parse response into Verdict.
        //
        // NOTE: The prompt will have already been filtered through the egress gate,
        // so secrets are redacted (C1, spec 07 H9).
        //
        // For now, this is a stub; actual HTTP calls live in the brain/server.
        Err(MemberError::Network {
            member: self.id.clone(),
            details: "HTTP adapter not yet implemented in this crate".to_string(),
        })
    }

    fn cost_estimate(&self, prompt: &Prompt) -> Cost {
        // Rough cost estimation based on prompt length.
        // This is a placeholder; real impl should consult provider pricing.
        let total_len = prompt.system.len() + prompt.user.len();
        let est_input_tokens = (total_len / 4).max(100) as u32; // Rough est: ~4 chars/token
        let est_output_tokens = 500; // Assume average response

        Cost {
            input_tokens: est_input_tokens,
            output_tokens: est_output_tokens,
            usd: (est_input_tokens as f64 * 0.00001) + (est_output_tokens as f64 * 0.00002),
        }
    }

    fn is_available(&self) -> bool {
        // In production: check circuit breaker state (C3)
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_member_from_env_missing_key_errors() {
        // Unset the key to test the error path
        env::remove_var("COUNCIL_ANTHROPIC_API_KEY");

        let result = HttpMember::from_env("anthropic", "claude-3-opus");
        assert!(result.is_err());
    }
}
