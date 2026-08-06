//! Core types for the Council (spec 05).

use serde::{Deserialize, Serialize};

/// The prompt sent to a council member (spec 05 §2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prompt {
    /// System-level instructions (e.g., "You are a fact-checker")
    pub system: String,
    /// User's question/claim to evaluate
    pub user: String,
    /// Which mode is calling this (influences tier used, C17)
    pub mode: Mode,
}

/// Council decision mode (spec 05 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Independent critique by each member, rotating chair (C4)
    Decide,
    /// Adversarial safety review (C6) — any flag blocks
    Security,
    /// Empirical fact-check (C7/C8) — requires resolvable sources
    Fact,
    /// Periodic re-audit of stored verified facts (C9)
    Audit,
}

/// Stance on a claim/decision (spec 05 C7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stance {
    /// Claim is supported (empirical) or decision is sound (decision)
    Supported,
    /// Claim is refuted or decision has a flaw
    Refuted,
    /// Cannot be resolved (insufficient evidence or ambiguous)
    Unverifiable,
}

/// Cost estimate/tracking (spec 05 C14).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Cost {
    /// Estimated input tokens
    pub input_tokens: u32,
    /// Estimated output tokens
    pub output_tokens: u32,
    /// Estimated USD cost
    pub usd: f64,
}

impl Cost {
    pub fn total_tokens(&self) -> u32 {
        self.input_tokens + self.output_tokens
    }
}

/// Verdict from a single council member (spec 05 §2).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Verdict {
    /// The member's position
    pub stance: Stance,
    /// Confidence [0.0, 1.0]
    pub confidence: f32,
    /// Required for fact-check (C7): a resolvable source URL or reference
    pub citation: Option<String>,
    /// Full reasoning text
    pub reasoning: String,
    /// For Mode 1 (Decide), any dissenting opinion (C5)
    pub dissent: Option<String>,
}

/// Execution context for a council call (spec 05 C16).
#[derive(Debug, Clone)]
pub struct CouncilCtx {
    /// Recursion depth in self-heal chain; >2 triggers refusal (C16)
    pub depth: usize,
    /// Trace ID for linking to ledger
    pub trace_id: String,
    /// When this call was initiated (RFC3339, G-09)
    pub initiated_at: String,
}

/// The final synthesized outcome of a council call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VerdictOutcome {
    /// Which mode was used
    pub source_mode: VerdictSourceMode,
    /// Synthesized decision (varies by mode)
    pub verdict: String,
    /// Chair (rotating for Mode 1)
    pub chair: Option<String>,
    /// All individual votes: member_id -> Verdict
    pub votes: std::collections::HashMap<String, Verdict>,
    /// Diversity flag (C12) — e.g., "low_diversity" if unanimous on contested claim
    pub diversity_flag: Option<String>,
    /// Cost in USD
    pub cost_usd: f64,
}

/// The mode-specific outcome (mirrors decision table schema).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VerdictSourceMode {
    /// Decide mode outcome
    Decide,
    /// Security review outcome
    Security,
    /// Fact-check outcome
    Fact,
    /// Audit outcome
    Audit,
}

impl From<Mode> for VerdictSourceMode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::Decide => VerdictSourceMode::Decide,
            Mode::Security => VerdictSourceMode::Security,
            Mode::Fact => VerdictSourceMode::Fact,
            Mode::Audit => VerdictSourceMode::Audit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_cost_total_tokens() {
        let cost = Cost {
            input_tokens: 100,
            output_tokens: 50,
            usd: 0.001,
        };
        assert_eq!(cost.total_tokens(), 150);
    }

    #[test]
    fn stance_serialize_deserialize() {
        let supported = Stance::Supported;
        let json = serde_json::to_string(&supported).unwrap();
        assert_eq!(json, "\"supported\"");
        let deserialized: Stance = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, Stance::Supported);
    }
}
