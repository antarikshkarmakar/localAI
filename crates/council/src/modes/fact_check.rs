//! Mode 3: Fact-check (spec 05 §3.3, C7/C8).
//!
//! Empirical claim verification:
//! - >=2 `supported` WITH at least one resolvable source, no `refuted` -> `verified`
//! - any `refuted`, or split -> `disputed`
//! - <2 members available -> `unverifiable` (never fabricate consensus, C8/G-12)
//! - A claim with 3x Supported but ZERO citations stays unverifiable (C7/C10, T1)

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::{CouncilCtx, CouncilMember, Prompt, Stance, Verdict};

/// Outcome of a fact-check call (stored as `verdict` in decisions table).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FactCheckVerdict {
    /// >=2 supported WITH at least one resolvable citation, no refuted (C8)
    Verified,
    /// Any refuted OR split votes (C8)
    Disputed,
    /// <2 members available OR no citations despite support (C7, T1)
    Unverifiable,
}

/// The result of performing a fact-check.
#[derive(Debug, Clone, PartialEq)]
pub struct FactCheckOutcome {
    pub verdict: FactCheckVerdict,
    /// Individual verdicts from members, keyed by member ID
    pub verdicts: HashMap<String, Verdict>,
    /// Which members were unavailable (G-12)
    pub unavailable: Vec<String>,
    /// Cost in USD
    pub cost_usd: f64,
}

/// Perform a fact-check on a claim (spec 05 C7/C8).
///
/// PRECONDITION: `prompt` has already been filtered through localai_security::filter()
/// (spec 07 H9, C1).
pub async fn check_fact(
    prompt: Prompt,
    ctx: &CouncilCtx,
    members: &[&dyn CouncilMember],
) -> Result<FactCheckOutcome, FactCheckError> {
    // Collect available members (C3, G-12)
    let available: Vec<_> = members
        .iter()
        .filter(|m| m.is_available())
        .copied()
        .collect();

    // Minimal set check (C8: never fabricate consensus from one voice)
    if available.len() < 2 {
        return Ok(FactCheckOutcome {
            verdict: FactCheckVerdict::Unverifiable,
            verdicts: HashMap::new(),
            unavailable: members
                .iter()
                .filter(|m| !m.is_available())
                .map(|m| m.id().to_string())
                .collect(),
            cost_usd: 0.0,
        });
    }

    // Ask each available member independently
    let mut verdicts = HashMap::new();
    let mut total_cost = 0.0;
    let mut errors = Vec::new();

    for member in &available {
        match member.ask(prompt.clone(), ctx).await {
            Ok(verdict) => {
                total_cost += member.cost_estimate(&prompt).usd;
                verdicts.insert(member.id().to_string(), verdict);
            }
            Err(e) => {
                errors.push(e);
            }
        }
    }

    // If we can't get at least 2 verdicts, unverifiable (G-12)
    if verdicts.len() < 2 {
        return Ok(FactCheckOutcome {
            verdict: FactCheckVerdict::Unverifiable,
            verdicts,
            unavailable: errors.iter().map(|_| "unknown".to_string()).collect(),
            cost_usd: total_cost,
        });
    }

    // Apply C7/C8 logic
    let outcome = analyze_verdicts(&verdicts);

    Ok(FactCheckOutcome {
        verdict: outcome,
        verdicts,
        unavailable: members
            .iter()
            .filter(|m| !m.is_available())
            .map(|m| m.id().to_string())
            .collect(),
        cost_usd: total_cost,
    })
}

/// Analyze verdicts to determine the final fact-check verdict.
/// Logic (spec 05 C7/C8):
/// - >=2 `supported` WITH at least one resolvable citation, no `refuted` -> `verified`
/// - any `refuted` -> `disputed`
/// - split (members disagree on Supported vs Refuted) -> `disputed`
/// - Otherwise -> `unverifiable` (including case where 3x Supported but zero citations, T1)
fn analyze_verdicts(verdicts: &HashMap<String, Verdict>) -> FactCheckVerdict {
    let mut supported_with_citation = 0;
    let mut refuted = 0;

    for verdict in verdicts.values() {
        match verdict.stance {
            Stance::Supported => {
                if verdict.citation.is_some() {
                    supported_with_citation += 1;
                }
                // (Supported without citation doesn't count toward verified, C7)
            }
            Stance::Refuted => {
                refuted += 1;
            }
            Stance::Unverifiable => {
                // Unverifiable doesn't block anything, just counts as not taking a stance
            }
        }
    }

    // Any refutation blocks verification (C8)
    if refuted > 0 {
        return FactCheckVerdict::Disputed;
    }

    // Check for split votes: if we have both Supported and Refuted, it's disputed
    // (but Unverifiable doesn't count as a split)
    if supported_with_citation > 0 && refuted > 0 {
        return FactCheckVerdict::Disputed;
    }

    // >=2 supported WITH citations, no refuted -> verified (C7, C8)
    if supported_with_citation >= 2 {
        return FactCheckVerdict::Verified;
    }

    // All other cases: unverifiable
    // This includes: 3x Supported but zero citations (T1), <2 with citations, etc.
    FactCheckVerdict::Unverifiable
}

/// Error in fact-checking.
#[derive(Debug)]
#[allow(dead_code)]
pub enum FactCheckError {
    /// All members unavailable
    AllMembersUnavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cost, MemberError};
    use async_trait::async_trait;

    /// Mock council member for testing.
    struct MockMember {
        id: String,
        verdict: Verdict,
        available: bool,
    }

    impl MockMember {
        fn new(id: &str, stance: Stance, confidence: f32, citation: Option<String>) -> Self {
            Self {
                id: id.to_string(),
                verdict: Verdict {
                    stance,
                    confidence,
                    citation,
                    reasoning: format!("{} reasoning", id),
                    dissent: None,
                },
                available: true,
            }
        }

        fn unavailable(id: &str) -> Self {
            Self {
                id: id.to_string(),
                verdict: Verdict {
                    stance: Stance::Unverifiable,
                    confidence: 0.0,
                    citation: None,
                    reasoning: String::new(),
                    dissent: None,
                },
                available: false,
            }
        }
    }

    #[async_trait]
    impl CouncilMember for MockMember {
        fn id(&self) -> &str {
            &self.id
        }

        fn model(&self) -> &str {
            "mock-model"
        }

        async fn ask(&self, _prompt: Prompt, _ctx: &CouncilCtx) -> Result<Verdict, MemberError> {
            if !self.available {
                return Err(MemberError::Unavailable {
                    member: self.id.clone(),
                });
            }
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

    fn make_prompt() -> Prompt {
        Prompt {
            system: "You are a fact-checker".to_string(),
            user: "Is water made of H2O?".to_string(),
            mode: crate::Mode::Fact,
        }
    }

    fn make_ctx() -> CouncilCtx {
        CouncilCtx {
            depth: 0,
            trace_id: "test-trace".to_string(),
            initiated_at: "2026-08-06T00:00:00Z".to_string(),
        }
    }

    // T1: 3x Supported, zero citations -> NOT verified (stays unverifiable/disputed)
    #[tokio::test]
    async fn t1_three_supported_zero_citations_is_unverifiable() {
        let m1 = MockMember::new("anthropic", Stance::Supported, 0.9, None);
        let m2 = MockMember::new("openai", Stance::Supported, 0.9, None);
        let m3 = MockMember::new("gemini", Stance::Supported, 0.9, None);
        let members: Vec<&dyn CouncilMember> = vec![&m1, &m2, &m3];

        let outcome = check_fact(make_prompt(), &make_ctx(), &members)
            .await
            .unwrap();

        // CRITICAL: no citations despite 3x support -> unverifiable, NOT verified (C7, C10)
        assert_eq!(outcome.verdict, FactCheckVerdict::Unverifiable);
    }

    // T1b: 2x Supported WITH citation, no refuted -> verified
    #[tokio::test]
    async fn t1b_two_supported_with_citation_is_verified() {
        let m1 = MockMember::new(
            "anthropic",
            Stance::Supported,
            0.9,
            Some("https://example.com/source1".to_string()),
        );
        let m2 = MockMember::new(
            "openai",
            Stance::Supported,
            0.9,
            Some("https://example.com/source2".to_string()),
        );
        let m3 = MockMember::new("gemini", Stance::Unverifiable, 0.5, None);
        let members: Vec<&dyn CouncilMember> = vec![&m1, &m2, &m3];

        let outcome = check_fact(make_prompt(), &make_ctx(), &members)
            .await
            .unwrap();

        assert_eq!(outcome.verdict, FactCheckVerdict::Verified);
    }

    // T3: only 1 member available -> unverifiable, no fabricated 1-voice consensus
    #[tokio::test]
    async fn t3_only_one_available_is_unverifiable() {
        let m1 = MockMember::new(
            "anthropic",
            Stance::Supported,
            0.9,
            Some("https://example.com".to_string()),
        );
        let m2 = MockMember::unavailable("openai");
        let m3 = MockMember::unavailable("gemini");
        let members: Vec<&dyn CouncilMember> = vec![&m1, &m2, &m3];

        let outcome = check_fact(make_prompt(), &make_ctx(), &members)
            .await
            .unwrap();

        assert_eq!(outcome.verdict, FactCheckVerdict::Unverifiable);
        assert_eq!(outcome.unavailable.len(), 2);
    }

    #[test]
    fn analyze_verdicts_refuted_makes_disputed() {
        let mut verdicts = HashMap::new();
        verdicts.insert(
            "a".to_string(),
            Verdict {
                stance: Stance::Supported,
                confidence: 0.9,
                citation: Some("url".to_string()),
                reasoning: "yes".to_string(),
                dissent: None,
            },
        );
        verdicts.insert(
            "b".to_string(),
            Verdict {
                stance: Stance::Refuted,
                confidence: 0.8,
                citation: Some("url".to_string()),
                reasoning: "no".to_string(),
                dissent: None,
            },
        );

        let result = analyze_verdicts(&verdicts);
        assert_eq!(result, FactCheckVerdict::Disputed);
    }
}
