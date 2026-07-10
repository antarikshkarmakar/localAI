//! MemoryGuard state machine (spec 01 §4, R11-R14).
//!
//! Transitions between memory budget levels based on RSS samples:
//! - **Normal**: RSS < soft_gb
//! - **Soft**: soft_gb ≤ RSS < hard_gb → pause background jobs + shrink caches
//! - **Hard**: hard_gb ≤ RSS < ceiling_gb → refuse new generations + shed load
//! - **Critical**: RSS ≥ ceiling_gb → emergency (kill workers, degraded mode)
//!
//! Level transitions fire actions ONCE per crossing. Downward recovery emits
//! recovery actions. Multi-level jumps fire in order soft→hard→critical.
//!
//! Pure state machine: observe() is deterministic, side-effect-free.
//! Brain executes returned GuardActions and logs ledger events.

use serde::{Deserialize, Serialize};

/// RSS sampler trait for dependency injection (spec 01 R5: no I/O in core).
/// Real implementation samples /proc/<pid>/smaps_rollup; tests inject values.
pub trait RssSampler {
    /// Current RSS in GB.
    fn sample_gb(&self) -> f64;
}

/// Memory budget levels (spec 01 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub enum GuardLevel {
    Normal = 0,
    Soft = 1,
    Hard = 2,
    Critical = 3,
}

/// Actions emitted by MemoryGuard on level transitions (spec 01 §4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GuardAction {
    /// Soft watermark crossed: pause background jobs + shrink caches.
    SoftWatermarkCrossed,
    /// Soft watermark recovered: resume background jobs.
    SoftWatermarkRecovered,
    /// Hard watermark crossed: refuse new generations + shed load.
    HardWatermarkCrossed,
    /// Hard watermark recovered: resume generations.
    HardWatermarkRecovered,
    /// Critical watermark crossed: emergency (kill workers, degraded mode).
    CriticalWatermarkCrossed,
    /// Critical watermark recovered: exit degraded mode.
    CriticalWatermarkRecovered,
}

/// MemoryGuard state machine (spec 01 §4, R11-R14).
#[derive(Debug, Clone)]
pub struct MemoryGuard {
    /// Configuration watermarks (GB).
    soft_gb: f64,
    hard_gb: f64,
    ceiling_gb: f64,

    /// Current level.
    level: GuardLevel,
}

impl MemoryGuard {
    /// Create a new MemoryGuard with the given watermarks.
    pub fn new(soft_gb: f64, hard_gb: f64, ceiling_gb: f64) -> Self {
        Self {
            soft_gb,
            hard_gb,
            ceiling_gb,
            level: GuardLevel::Normal,
        }
    }

    /// Get the current memory budget level.
    pub fn level(&self) -> GuardLevel {
        self.level
    }

    /// Observe a new RSS sample and return actions to execute.
    ///
    /// Deterministic and side-effect-free. Transitions fire actions ONCE per
    /// crossing. Multi-level jumps fire in order soft→hard→critical.
    /// Recovery (downward) also fires in soft→hard→critical order.
    pub fn observe(&mut self, sample: &dyn RssSampler) -> Vec<GuardAction> {
        let rss = sample.sample_gb();
        let new_level = self.classify_level(rss);

        if new_level == self.level {
            return Vec::new();
        }

        let mut actions = Vec::new();

        if new_level > self.level {
            // Ascending: fire in order (soft → hard → critical).
            for level in (self.level as u8 + 1)..=new_level as u8 {
                match level {
                    1 => actions.push(GuardAction::SoftWatermarkCrossed),
                    2 => actions.push(GuardAction::HardWatermarkCrossed),
                    3 => actions.push(GuardAction::CriticalWatermarkCrossed),
                    _ => {}
                }
            }
        } else {
            // Descending: fire in order (soft → hard → critical), but for recovered levels.
            for level in (new_level as u8 + 1)..=self.level as u8 {
                match level {
                    1 => actions.push(GuardAction::SoftWatermarkRecovered),
                    2 => actions.push(GuardAction::HardWatermarkRecovered),
                    3 => actions.push(GuardAction::CriticalWatermarkRecovered),
                    _ => {}
                }
            }
        }

        self.level = new_level;
        actions
    }

    /// Classify RSS into a level.
    fn classify_level(&self, rss: f64) -> GuardLevel {
        if rss >= self.ceiling_gb {
            GuardLevel::Critical
        } else if rss >= self.hard_gb {
            GuardLevel::Hard
        } else if rss >= self.soft_gb {
            GuardLevel::Soft
        } else {
            GuardLevel::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock sampler for testing.
    struct MockSampler {
        gb: f64,
    }

    impl RssSampler for MockSampler {
        fn sample_gb(&self) -> f64 {
            self.gb
        }
    }

    // T2 part 1: Single watermark crossing (Normal → Soft).
    #[test]
    fn single_crossing_soft() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        assert_eq!(guard.level(), GuardLevel::Normal);

        let sampler = MockSampler { gb: 19.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(actions, vec![GuardAction::SoftWatermarkCrossed]);
        assert_eq!(guard.level(), GuardLevel::Soft);
    }

    // T2 part 2: Multi-level jumps fire in order.
    #[test]
    fn multi_level_jump_normal_to_hard() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 21.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::SoftWatermarkCrossed,
                GuardAction::HardWatermarkCrossed
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Hard);
    }

    #[test]
    fn multi_level_jump_normal_to_critical() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 22.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::SoftWatermarkCrossed,
                GuardAction::HardWatermarkCrossed,
                GuardAction::CriticalWatermarkCrossed
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Critical);
    }

    #[test]
    fn multi_level_jump_soft_to_critical() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 19.5 };
        guard.observe(&sampler); // Soft
        assert_eq!(guard.level(), GuardLevel::Soft);

        let sampler = MockSampler { gb: 22.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::HardWatermarkCrossed,
                GuardAction::CriticalWatermarkCrossed
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Critical);
    }

    // T2 part 3: No re-fire at same level.
    #[test]
    fn no_re_fire_same_level() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 19.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(actions, vec![GuardAction::SoftWatermarkCrossed]);

        let sampler = MockSampler { gb: 19.8 };
        let actions = guard.observe(&sampler);
        assert_eq!(actions, vec![]);
        assert_eq!(guard.level(), GuardLevel::Soft);
    }

    // T2 part 4: Downward recovery emits recovery actions.
    #[test]
    fn recovery_soft_to_normal() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 19.5 };
        guard.observe(&sampler); // Soft
        assert_eq!(guard.level(), GuardLevel::Soft);

        let sampler = MockSampler { gb: 18.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(actions, vec![GuardAction::SoftWatermarkRecovered]);
        assert_eq!(guard.level(), GuardLevel::Normal);
    }

    #[test]
    fn recovery_hard_to_normal() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 21.5 };
        guard.observe(&sampler); // Hard
        assert_eq!(guard.level(), GuardLevel::Hard);

        let sampler = MockSampler { gb: 18.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::SoftWatermarkRecovered,
                GuardAction::HardWatermarkRecovered
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Normal);
    }

    #[test]
    fn recovery_critical_to_normal() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 22.5 };
        guard.observe(&sampler); // Critical
        assert_eq!(guard.level(), GuardLevel::Critical);

        let sampler = MockSampler { gb: 18.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::SoftWatermarkRecovered,
                GuardAction::HardWatermarkRecovered,
                GuardAction::CriticalWatermarkRecovered
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Normal);
    }

    #[test]
    fn recovery_critical_to_soft() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 22.5 };
        guard.observe(&sampler); // Critical
        assert_eq!(guard.level(), GuardLevel::Critical);

        let sampler = MockSampler { gb: 19.5 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::HardWatermarkRecovered,
                GuardAction::CriticalWatermarkRecovered
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Soft);
    }

    // T2 part 5: Boundary-exact values.
    #[test]
    fn boundary_exact_soft() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 19.0 };
        let actions = guard.observe(&sampler);
        assert_eq!(actions, vec![GuardAction::SoftWatermarkCrossed]);
        assert_eq!(guard.level(), GuardLevel::Soft);
    }

    #[test]
    fn boundary_exact_hard() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 21.0 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::SoftWatermarkCrossed,
                GuardAction::HardWatermarkCrossed
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Hard);
    }

    #[test]
    fn boundary_exact_ceiling() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 22.0 };
        let actions = guard.observe(&sampler);
        assert_eq!(
            actions,
            vec![
                GuardAction::SoftWatermarkCrossed,
                GuardAction::HardWatermarkCrossed,
                GuardAction::CriticalWatermarkCrossed
            ]
        );
        assert_eq!(guard.level(), GuardLevel::Critical);
    }

    #[test]
    fn boundary_just_below_soft() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 18.999 };
        let actions = guard.observe(&sampler);
        assert_eq!(actions, vec![]);
        assert_eq!(guard.level(), GuardLevel::Normal);
    }

    // Verify recovery ordering (symmetric with escalation: soft → hard → critical).
    #[test]
    fn recovery_preserves_soft_to_hard_to_critical_order() {
        let mut guard = MemoryGuard::new(19.0, 21.0, 22.0);
        let sampler = MockSampler { gb: 22.5 };
        guard.observe(&sampler); // Critical

        let sampler = MockSampler { gb: 18.5 };
        let actions = guard.observe(&sampler);

        // Recovery fires in same order as escalation: soft → hard → critical.
        assert_eq!(actions[0], GuardAction::SoftWatermarkRecovered);
        assert_eq!(actions[1], GuardAction::HardWatermarkRecovered);
        assert_eq!(actions[2], GuardAction::CriticalWatermarkRecovered);
    }
}
