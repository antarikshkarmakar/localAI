//! Test utilities and mock members for council testing.

use crate::{Cost, CouncilCtx, CouncilMember, MemberError, Prompt, Stance, Verdict};
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

/// Mock council member that records all calls (for testing T5/T7).
#[derive(Clone)]
pub struct RecordingMember {
    id: String,
    model: String,
    verdict: Verdict,
    available: bool,
    /// Recorded prompts sent to this member (for asserting redaction, T7)
    calls: Arc<Mutex<Vec<Prompt>>>,
}

impl RecordingMember {
    pub fn new(
        id: &str,
        model: &str,
        stance: Stance,
        confidence: f32,
        citation: Option<String>,
    ) -> Self {
        Self {
            id: id.to_string(),
            model: model.to_string(),
            verdict: Verdict {
                stance,
                confidence,
                citation,
                reasoning: format!("{} says it looks good", id),
                dissent: None,
            },
            available: true,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Mark this member as unavailable (for testing T3, T5).
    pub fn set_unavailable(mut self) -> Self {
        self.available = false;
        self
    }

    /// Get all recorded prompts (for asserting no secrets leaked, T7).
    pub fn get_calls(&self) -> Vec<Prompt> {
        self.calls.lock().unwrap().clone()
    }

    /// Assert no calls were made (for testing T5 cost ceiling).
    pub fn assert_no_calls(&self) {
        let calls = self.calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "expected no calls, but got {}",
            calls.len()
        );
    }
}

#[async_trait]
impl CouncilMember for RecordingMember {
    fn id(&self) -> &str {
        &self.id
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn ask(&self, prompt: Prompt, _ctx: &CouncilCtx) -> Result<Verdict, MemberError> {
        if !self.available {
            return Err(MemberError::Unavailable {
                member: self.id.clone(),
            });
        }

        // Record the prompt (for testing redaction)
        self.calls.lock().unwrap().push(prompt);

        Ok(self.verdict.clone())
    }

    fn cost_estimate(&self, _prompt: &Prompt) -> Cost {
        Cost {
            input_tokens: 100,
            output_tokens: 50,
            usd: 0.001,
        }
    }

    fn is_available(&self) -> bool {
        self.available
    }
}
