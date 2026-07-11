//! Watchdog: tiny supervisor that restarts Brain on crash/hang.
//!
//! Spec 09 H9: "Watchdog is dumb by design — it only restarts; it never makes decisions."
//!
//! ## Architecture
//!
//! - **Heartbeat protocol**: Brain writes monotonically increasing counter to a file (atomic via temp+rename).
//! - **WatchdogPolicy**: Pure logic (no I/O) that ingests observations `(poll_index, counter_value)` and emits `Action::None | Action::Restart`.
//! - **Backoff**: Exponential delay on crash-loop (cap it to avoid spinning the box).
//! - **Binary**: Read heartbeat file, feed policy, exec process control (kill + spawn).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Action emitted by WatchdogPolicy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Action {
    /// Everything is healthy, wait for next poll.
    None,
    /// Process is hung or crashed, restart it.
    Restart,
}

/// Configuration for the watchdog policy.
#[derive(Debug, Clone)]
pub struct PolicyConfig {
    /// Number of consecutive missed heartbeats before declaring crash.
    pub max_missed_polls: usize,
    /// Initial backoff delay (ms) after a restart.
    pub backoff_initial_ms: u64,
    /// Maximum backoff delay (ms) cap to prevent unbounded growth.
    pub backoff_max_ms: u64,
    /// Backoff multiplier for exponential growth.
    pub backoff_multiplier: f64,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            max_missed_polls: 3,
            backoff_initial_ms: 100,
            backoff_max_ms: 30_000,
            backoff_multiplier: 2.0,
        }
    }
}

/// Pure logic state machine for watchdog policy decisions.
///
/// Spec 09 H9: Watchdog is dumb by design — only tracks heartbeat counter + missed polls,
/// emits restart decisions, resets state after restart. No I/O, no ledger writes.
/// This is the "small trusted computing base" (spec 01 §1, spec 09 §5).
#[derive(Debug)]
pub struct WatchdogPolicy {
    config: PolicyConfig,
    last_counter: Option<u64>,
    missed_polls: usize,
    restart_backoff_ms: u64,
    consecutive_restarts: usize,
}

impl WatchdogPolicy {
    /// Create a new policy with given config.
    pub fn new(config: PolicyConfig) -> Self {
        let restart_backoff_ms = config.backoff_initial_ms;
        Self {
            config,
            last_counter: None,
            missed_polls: 0,
            restart_backoff_ms,
            consecutive_restarts: 0,
        }
    }

    /// Observe the current heartbeat counter (or None if file missing/unreadable).
    ///
    /// Returns the action to take (None or Restart).
    /// If a Restart is returned, the caller is responsible for killing/spawning the process.
    /// After a successful restart, the caller should call `acknowledge_restart()`.
    pub fn observe(&mut self, counter: Option<u64>) -> Action {
        match counter {
            Some(c) => {
                match self.last_counter {
                    None => {
                        // First observation ever.
                        self.last_counter = Some(c);
                        self.missed_polls = 0;
                        Action::None
                    }
                    Some(prev) if c > prev => {
                        // Counter advanced: process is healthy.
                        self.last_counter = Some(c);
                        self.missed_polls = 0;
                        Action::None
                    }
                    Some(prev) if c == prev => {
                        // Counter stalled: process may be hung.
                        self.missed_polls += 1;
                        if self.missed_polls >= self.config.max_missed_polls {
                            self.missed_polls = 0; // Reset for next cycle
                            self.consecutive_restarts += 1;
                            Action::Restart
                        } else {
                            Action::None
                        }
                    }
                    Some(_prev) => {
                        // Counter went backward (impossible in normal operation, but treat as hang).
                        self.missed_polls += 1;
                        if self.missed_polls >= self.config.max_missed_polls {
                            self.missed_polls = 0;
                            self.consecutive_restarts += 1;
                            Action::Restart
                        } else {
                            Action::None
                        }
                    }
                }
            }
            None => {
                // Missing or unreadable heartbeat file: treat as a miss.
                self.missed_polls += 1;
                if self.missed_polls >= self.config.max_missed_polls {
                    self.missed_polls = 0;
                    self.consecutive_restarts += 1;
                    Action::Restart
                } else {
                    Action::None
                }
            }
        }
    }

    /// Acknowledge a successful restart. Resets miss count and optimizes backoff for next time.
    ///
    /// Call this after you've spawned a new process in response to a Restart action.
    pub fn acknowledge_restart(&mut self) {
        self.last_counter = None;
        self.missed_polls = 0;
        // Backoff grows AFTER being used: the delay applied for restart N is
        // the pre-growth value; the next crash waits longer. Capped at max.
        let grown = ((self.restart_backoff_ms as f64) * self.config.backoff_multiplier) as u64;
        self.restart_backoff_ms = grown.min(self.config.backoff_max_ms);
        // consecutive_restarts decays on sustained health (acknowledge_health).
    }

    /// After a healthy period (counter advanced), decay consecutive restarts and backoff.
    ///
    /// Call this when you detect sustained health (e.g., after 10 consecutive healthy polls).
    pub fn acknowledge_health(&mut self) {
        if self.consecutive_restarts > 0 {
            self.consecutive_restarts = self.consecutive_restarts.saturating_sub(1);
        }
        if self.restart_backoff_ms > self.config.backoff_initial_ms {
            self.restart_backoff_ms = self
                .restart_backoff_ms
                .saturating_sub(self.config.backoff_initial_ms / 2);
            self.restart_backoff_ms = self.restart_backoff_ms.max(self.config.backoff_initial_ms);
        }
    }

    /// Get the current backoff delay in milliseconds.
    pub fn current_backoff_ms(&self) -> u64 {
        self.restart_backoff_ms
    }

    /// Get the current number of consecutive restarts without sustained health.
    pub fn consecutive_restarts(&self) -> usize {
        self.consecutive_restarts
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Action::None => write!(f, "None"),
            Action::Restart => write!(f, "Restart"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_first_observation() {
        let mut policy = WatchdogPolicy::new(PolicyConfig::default());
        let action = policy.observe(Some(1));
        assert_eq!(action, Action::None, "first observation should be healthy");
    }

    #[test]
    fn policy_counter_advancing() {
        let mut policy = WatchdogPolicy::new(PolicyConfig::default());
        assert_eq!(policy.observe(Some(1)), Action::None);
        assert_eq!(policy.observe(Some(2)), Action::None);
        assert_eq!(policy.observe(Some(3)), Action::None);
        // Counter advancing means process is alive.
    }

    #[test]
    fn policy_counter_stalled_triggers_restart() {
        let config = PolicyConfig {
            max_missed_polls: 3,
            ..Default::default()
        };
        let mut policy = WatchdogPolicy::new(config);

        // First observation
        assert_eq!(policy.observe(Some(1)), Action::None);

        // Counter doesn't advance: first miss
        assert_eq!(policy.observe(Some(1)), Action::None);

        // Second miss
        assert_eq!(policy.observe(Some(1)), Action::None);

        // Third miss: triggers restart
        assert_eq!(policy.observe(Some(1)), Action::Restart);
    }

    #[test]
    fn policy_missing_heartbeat_counted_as_miss() {
        let config = PolicyConfig {
            max_missed_polls: 2,
            ..Default::default()
        };
        let mut policy = WatchdogPolicy::new(config);

        assert_eq!(policy.observe(Some(1)), Action::None);

        // Missing file: first miss
        assert_eq!(policy.observe(None), Action::None);

        // Missing again: second miss, triggers restart
        assert_eq!(policy.observe(None), Action::Restart);
    }

    #[test]
    fn policy_recovery_after_restart() {
        let config = PolicyConfig {
            max_missed_polls: 2,
            ..Default::default()
        };
        let mut policy = WatchdogPolicy::new(config);

        // Start up
        assert_eq!(policy.observe(Some(100)), Action::None);

        // Stall out
        assert_eq!(policy.observe(Some(100)), Action::None);
        assert_eq!(policy.observe(Some(100)), Action::Restart);

        // Acknowledge restart
        policy.acknowledge_restart();

        // New process starts, counter advances: should be healthy
        assert_eq!(policy.observe(Some(1)), Action::None);
        assert_eq!(policy.observe(Some(2)), Action::None);
    }

    #[test]
    fn policy_crash_loop_backoff_grows() {
        let config = PolicyConfig {
            max_missed_polls: 1,
            backoff_initial_ms: 100,
            backoff_max_ms: 10_000,
            backoff_multiplier: 2.0,
        };
        let mut policy = WatchdogPolicy::new(config);

        // First crash
        policy.observe(Some(1));
        assert_eq!(policy.observe(Some(1)), Action::Restart);
        assert_eq!(policy.current_backoff_ms(), 100);
        policy.acknowledge_restart();

        // Second crash in a row
        policy.observe(Some(2));
        assert_eq!(policy.observe(Some(2)), Action::Restart);
        assert_eq!(policy.current_backoff_ms(), 200, "backoff should double");
        policy.acknowledge_restart();

        // Third crash
        policy.observe(Some(3));
        assert_eq!(policy.observe(Some(3)), Action::Restart);
        assert_eq!(
            policy.current_backoff_ms(),
            400,
            "backoff should double again"
        );
    }

    #[test]
    fn policy_backoff_caps() {
        let config = PolicyConfig {
            max_missed_polls: 1,
            backoff_initial_ms: 100,
            backoff_max_ms: 500,
            backoff_multiplier: 2.0,
        };
        let mut policy = WatchdogPolicy::new(config);

        // Trigger multiple crashes to exceed cap
        for _ in 0..10 {
            policy.observe(Some(1));
            policy.observe(Some(1));
            if matches!(policy.observe(Some(1)), Action::Restart) {
                policy.acknowledge_restart();
            }
        }

        assert!(
            policy.current_backoff_ms() <= 500,
            "backoff should not exceed max"
        );
    }

    #[test]
    fn policy_consecutive_restart_count() {
        let config = PolicyConfig {
            max_missed_polls: 1,
            ..Default::default()
        };
        let mut policy = WatchdogPolicy::new(config);

        assert_eq!(policy.consecutive_restarts(), 0);

        policy.observe(Some(1));
        assert_eq!(policy.observe(Some(1)), Action::Restart);
        assert_eq!(policy.consecutive_restarts(), 1);

        policy.acknowledge_restart();
        policy.observe(Some(2));
        assert_eq!(policy.observe(Some(2)), Action::Restart);
        assert_eq!(policy.consecutive_restarts(), 2);
    }

    #[test]
    fn policy_health_decay() {
        let config = PolicyConfig {
            max_missed_polls: 1,
            backoff_initial_ms: 100,
            backoff_max_ms: 1000,
            backoff_multiplier: 2.0,
        };
        let mut policy = WatchdogPolicy::new(config);

        // Build up backoff
        for _ in 0..3 {
            policy.observe(Some(1));
            policy.observe(Some(1));
            let _ = policy.observe(Some(1));
            policy.acknowledge_restart();
        }

        let high_backoff = policy.current_backoff_ms();
        assert!(high_backoff > 100);

        // Decay it
        policy.acknowledge_health();
        assert!(
            policy.current_backoff_ms() < high_backoff,
            "backoff should decay toward initial"
        );
    }
}
