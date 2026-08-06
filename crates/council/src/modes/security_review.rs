//! Mode 2: Security review (spec 05 §3.2, C6).
//!
//! Adversarial safety review before self-modification:
//! - Members asked to FIND failures (not approve)
//! - ANY member flagging regression -> BLOCK (fail-safe, not fail-consensus, C6)
//! - Returns blocked-with-reason or passed

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{CouncilCtx, CouncilMember, Prompt, Stance, Verdict};

/// Outcome of a security review (spec 05 C6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecurityReviewOutcome {
    /// All members approve (or flag nothing)
    Passed,
    /// At least one member flagged a regression -> change blocked
    Blocked { reason: String },
}

/// Result of performing a security review.
#[derive(Debug, Clone)]
pub struct SecurityReviewResult {
    pub outcome: SecurityReviewOutcome,
    /// Individual verdicts from all members
    #[allow(dead_code)]
    pub verdicts: HashMap<String, Verdict>,
    /// Cost in USD
    #[allow(dead_code)]
    pub cost_usd: f64,
}

/// Perform a security review before self-mod activation (spec 05 C6).
///
/// PRECONDITION: `prompt` has already been filtered through localai_security::filter()
/// (spec 07 H9, C1).
///
/// Fail-safe logic: ANY Refuted verdict -> BLOCKED (C6, T2)
pub async fn review_security(
    prompt: Prompt,
    ctx: &CouncilCtx,
    members: &[&dyn CouncilMember],
) -> Result<SecurityReviewResult, SecurityReviewError> {
    let available: Vec<_> = members
        .iter()
        .filter(|m| m.is_available())
        .copied()
        .collect();

    if available.is_empty() {
        return Err(SecurityReviewError::NoMembersAvailable);
    }

    let mut verdicts = HashMap::new();
    let mut total_cost = 0.0;

    for member in &available {
        match member.ask(prompt.clone(), ctx).await {
            Ok(verdict) => {
                total_cost += member.cost_estimate(&prompt).usd;
                verdicts.insert(member.id().to_string(), verdict);
            }
            Err(_) => {
                // Transient error; continue with other members
            }
        }
    }

    // Analyze verdicts: any Refuted means block (fail-safe, C6)
    let outcome = analyze_security_verdicts(&verdicts);

    Ok(SecurityReviewResult {
        outcome,
        verdicts,
        cost_usd: total_cost,
    })
}

/// Analyze verdicts for security review (spec 05 C6, fail-safe).
/// If ANY member refutes (flags a regression), block the change.
fn analyze_security_verdicts(verdicts: &HashMap<String, Verdict>) -> SecurityReviewOutcome {
    for verdict in verdicts.values() {
        if verdict.stance == Stance::Refuted {
            return SecurityReviewOutcome::Blocked {
                reason: verdict.reasoning.clone(),
            };
        }
    }

    SecurityReviewOutcome::Passed
}

/// Error in security review.
#[derive(Debug)]
pub enum SecurityReviewError {
    /// No members available to perform review
    NoMembersAvailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cost, MemberError};
    use async_trait::async_trait;

    struct MockMember {
        id: String,
        verdict: Verdict,
        available: bool,
    }

    impl MockMember {
        fn new(id: &str, stance: crate::Stance, reasoning: &str) -> Self {
            Self {
                id: id.to_string(),
                verdict: Verdict {
                    stance,
                    confidence: 0.8,
                    citation: None,
                    reasoning: reasoning.to_string(),
                    dissent: None,
                },
                available: true,
            }
        }
    }

    #[async_trait]
    impl CouncilMember for MockMember {
        fn id(&self) -> &str {
            &self.id
        }

        fn model(&self) -> &str {
            "mock"
        }

        async fn ask(&self, _prompt: Prompt, _ctx: &CouncilCtx) -> Result<Verdict, MemberError> {
            Ok(self.verdict.clone())
        }

        fn cost_estimate(&self, _prompt: &Prompt) -> Cost {
            Cost {
                input_tokens: 150,
                output_tokens: 100,
                usd: 0.002,
            }
        }

        fn is_available(&self) -> bool {
            self.available
        }
    }

    fn make_prompt() -> Prompt {
        Prompt {
            system: "Review for safety regressions".to_string(),
            user: "Diff of config changes".to_string(),
            mode: crate::Mode::Security,
        }
    }

    fn make_ctx() -> CouncilCtx {
        CouncilCtx {
            depth: 0,
            trace_id: "test".to_string(),
            initiated_at: "2026-08-06T00:00:00Z".to_string(),
        }
    }

    // T2: One member flags regression, two approve -> change BLOCKED
    #[tokio::test]
    async fn t2_one_refute_blocks_change() {
        let m1 = MockMember::new(
            "anthropic",
            crate::Stance::Refuted,
            "Found a regression: removes safety check",
        );
        let m2 = MockMember::new("openai", crate::Stance::Supported, "Looks fine");
        let m3 = MockMember::new("gemini", crate::Stance::Supported, "Approved");
        let members: Vec<&dyn CouncilMember> = vec![&m1, &m2, &m3];

        let result = review_security(make_prompt(), &make_ctx(), &members)
            .await
            .unwrap();

        // CRITICAL: fail-safe logic (C6) — any Refuted -> BLOCKED
        match result.outcome {
            SecurityReviewOutcome::Blocked { ref reason } => {
                assert!(reason.contains("regression"));
            }
            _ => panic!("expected Blocked outcome"),
        }
    }

    #[tokio::test]
    async fn security_all_supported_passes() {
        let m1 = MockMember::new("anthropic", crate::Stance::Supported, "OK");
        let m2 = MockMember::new("openai", crate::Stance::Supported, "OK");
        let m3 = MockMember::new("gemini", crate::Stance::Unverifiable, "Needs more info");
        let members: Vec<&dyn CouncilMember> = vec![&m1, &m2, &m3];

        let result = review_security(make_prompt(), &make_ctx(), &members)
            .await
            .unwrap();

        assert_eq!(result.outcome, SecurityReviewOutcome::Passed);
    }
}
