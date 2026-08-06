//! Chair rotation for Mode 1 (Decide) decisions (spec 05 C4, T8).
//!
//! The chair role rotates per call so no single provider dominates framing.

/// Chair rotator for Mode 1 (Decide) decisions (spec 05 C4, T8).
///
/// Ensures that no provider chairs twice in a row.
pub struct ChairRotator {
    /// History of chairs used (most recent first)
    chair_history: Vec<String>,
    /// Maximum history to retain
    max_history: usize,
}

impl ChairRotator {
    pub fn new() -> Self {
        Self {
            chair_history: Vec::new(),
            max_history: 1, // Just track the last chair to avoid repeats
        }
    }

    /// Select the next chair from available members.
    /// Prefers to skip the most recent chair (T8).
    pub fn select_next_chair(&mut self, available_member_ids: &[&str]) -> Option<String> {
        if available_member_ids.is_empty() {
            return None;
        }

        // If no history, pick the first
        if self.chair_history.is_empty() {
            let selected = available_member_ids[0].to_string();
            self.chair_history.push(selected.clone());
            return Some(selected);
        }

        // Prefer a member who hasn't been chair recently
        let last_chair = &self.chair_history[0];
        let next = available_member_ids
            .iter()
            .find(|id| *id != last_chair)
            .or(available_member_ids.first())
            .map(|id| id.to_string());

        if let Some(ref chair) = next {
            // Add to history and trim
            self.chair_history.insert(0, chair.clone());
            if self.chair_history.len() > self.max_history {
                self.chair_history.pop();
            }
        }

        next
    }
}

impl Default for ChairRotator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t8_chair_rotates_across_calls() {
        let mut rotator = ChairRotator::new();

        let members = vec!["anthropic", "openai", "gemini"];

        // First call
        let chair1 = rotator.select_next_chair(&members);
        assert_eq!(chair1, Some("anthropic".to_string()));

        // Second call — should skip anthropic
        let chair2 = rotator.select_next_chair(&members);
        assert_ne!(chair2, Some("anthropic".to_string()));

        // Third call — should skip whoever was chair2
        let chair3 = rotator.select_next_chair(&members);
        assert_ne!(chair3, chair2);
    }

    #[test]
    fn chair_first_call_picks_first() {
        let mut rotator = ChairRotator::new();
        let members = vec!["anthropic", "openai", "gemini"];

        let chair = rotator.select_next_chair(&members);
        assert_eq!(chair, Some("anthropic".to_string()));
    }

    #[test]
    fn chair_empty_members_returns_none() {
        let mut rotator = ChairRotator::new();
        let members: Vec<&str> = vec![];

        let chair = rotator.select_next_chair(&members);
        assert_eq!(chair, None);
    }

    #[test]
    fn chair_single_member_always_selected() {
        let mut rotator = ChairRotator::new();
        let members = vec!["anthropic"];

        let chair1 = rotator.select_next_chair(&members);
        let chair2 = rotator.select_next_chair(&members);

        assert_eq!(chair1, Some("anthropic".to_string()));
        assert_eq!(chair2, Some("anthropic".to_string()));
    }
}
