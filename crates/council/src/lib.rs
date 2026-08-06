//! LLM Council — external advisory for high-stakes decisions (spec 05).
//!
//! Provides a voting/gating engine for three cloud LLMs (Claude, OpenAI, Gemini):
//! - Mode 1 (COUNCIL_DECIDE): independent critique, rotating chair, preserved dissent
//! - Mode 2 (COUNCIL_SECURITY): fail-safe review (any flag blocks)
//! - Mode 3 (COUNCIL_FACT): empirical claim verification via resolvable sources
//! - Cost governance, secret redaction at egress, circuit-breaker per provider
//!
//! All outbound prompts pass through localai_security::filter() before any provider call (C1/T7).

mod adapters;
mod chair;
mod egress;
mod governor;
mod member;
mod modes;
pub mod spend;
mod types;

pub use adapters::HttpMember;
pub use chair::ChairRotator;
pub use egress::{apply_egress_filter, has_secrets};
pub use governor::{check_cost_and_depth, CostCheckResult, CostConfig};
pub use member::{CouncilMember, MemberError};
pub use modes::fact_check::{check_fact, FactCheckOutcome, FactCheckVerdict};
pub use modes::security_review::{review_security, SecurityReviewOutcome};
pub use spend::{record_call, record_decision, snapshot, DecisionRecord, SpendSnapshot};
pub use types::{
    Cost, CouncilCtx, Mode, Prompt, Stance, Verdict, VerdictOutcome, VerdictSourceMode,
};

/// Re-export for test convenience
#[cfg(test)]
pub mod test_helpers;
