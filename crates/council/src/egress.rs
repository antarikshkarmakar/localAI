//! Egress gate with secret filtering (spec 05 C1, spec 07 H9, GAPS G-14).
//!
//! Every outbound prompt MUST be filtered through localai_security::filter()
//! before any provider call. This module enforces that invariant.

use crate::Prompt;
use localai_security::Filtered;

/// Filter a prompt through the security filter before sending to a provider.
/// Returns the filtered prompt and the filter report.
///
/// PRECONDITION: This is the ONLY path outbound prompts should take (spec 07 H9, C1).
pub fn apply_egress_filter(prompt: &Prompt) -> (String, Filtered) {
    // Combine system + user into a single string for filtering (secrets could be in either)
    let full_prompt = format!("{}\n{}", prompt.system, prompt.user);
    let filtered = localai_security::filter(&full_prompt);

    // Reconstruct the filtered prompt (for now, return the full filtered text)
    (filtered.text.clone(), filtered)
}

/// Check if a filter result indicates a secret leak attempt (C1).
/// Returns true if secrets were detected.
pub fn has_secrets(filtered: &Filtered) -> bool {
    filtered.is_dirty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Mode, Prompt};

    #[test]
    fn t7_secret_in_prompt_is_redacted() {
        // T7: Secret accidentally in evidence excerpt -> redacted by egress
        let prompt = Prompt {
            system: "You are a fact-checker".to_string(),
            user: "Is this true? API key sk-ABCDEFGHIJKLMNOPQR was found".to_string(),
            mode: Mode::Fact,
        };

        let (filtered_text, filtered) = apply_egress_filter(&prompt);

        // The secret should be redacted
        assert!(!filtered_text.contains("sk-ABCDEFGHIJKLMNOPQR"));
        assert!(filtered_text.contains("[REDACTED]"));
        assert!(has_secrets(&filtered));

        // The filter report must not contain the secret (spec 11 S4)
        let report = format!("{:?}", filtered);
        assert!(!report.contains("ABCDEFGHIJKLMNOPQR"));
    }

    #[test]
    fn clean_prompt_passes_through() {
        let prompt = Prompt {
            system: "You are a fact-checker".to_string(),
            user: "Is water H2O?".to_string(),
            mode: Mode::Fact,
        };

        let (filtered_text, filtered) = apply_egress_filter(&prompt);

        assert!(!has_secrets(&filtered));
        assert!(filtered_text.contains("Is water H2O"));
    }
}
