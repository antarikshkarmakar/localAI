//! Cost governance and recursion guard (spec 05 C14/C15/C16).
//!
//! Tracks spending, enforces daily/monthly ceilings, and prevents deep recursion.

use serde::{Deserialize, Serialize};

/// Configuration for council cost control (spec 05 C14/C15/C16).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    /// Daily spending ceiling in USD (e.g., 10.0)
    pub daily_ceiling_usd: f64,
    /// Monthly spending ceiling in USD (e.g., 200.0)
    pub monthly_ceiling_usd: f64,
    /// Maximum recursion depth before refusing (C16, G-07)
    pub max_depth: usize,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            daily_ceiling_usd: 10.0,
            monthly_ceiling_usd: 200.0,
            max_depth: 2,
        }
    }
}

/// Result of a pre-flight cost check (spec 05 C15).
#[derive(Debug, Clone, PartialEq)]
pub enum CostCheckResult {
    /// Cost is within budget; proceed
    OK,
    /// Cost would exceed daily ceiling
    DailyLimitExceeded { current: f64, ceiling: f64 },
    /// Cost would exceed monthly ceiling
    MonthlyLimitExceeded { current: f64, ceiling: f64 },
    /// Recursion depth exceeds max (spec 05 C16)
    DepthExceeded { depth: usize, max: usize },
}

/// Check if a council call can proceed (spec 05 C15/C16).
pub fn check_cost_and_depth(
    estimated_cost: f64,
    current_daily: f64,
    current_monthly: f64,
    current_depth: usize,
    config: &CostConfig,
) -> CostCheckResult {
    // Check recursion depth first (C16, T6)
    if current_depth > config.max_depth {
        return CostCheckResult::DepthExceeded {
            depth: current_depth,
            max: config.max_depth,
        };
    }

    // Check daily ceiling (C15, T5)
    if current_daily + estimated_cost > config.daily_ceiling_usd {
        return CostCheckResult::DailyLimitExceeded {
            current: current_daily,
            ceiling: config.daily_ceiling_usd,
        };
    }

    // Check monthly ceiling (C15)
    if current_monthly + estimated_cost > config.monthly_ceiling_usd {
        return CostCheckResult::MonthlyLimitExceeded {
            current: current_monthly,
            ceiling: config.monthly_ceiling_usd,
        };
    }

    CostCheckResult::OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t5_cost_ceiling_reached_returns_unavailable() {
        let config = CostConfig {
            daily_ceiling_usd: 10.0,
            monthly_ceiling_usd: 200.0,
            max_depth: 2,
        };

        let result = check_cost_and_depth(
            5.0,   // estimated cost
            9.0,   // current daily spend
            100.0, // current monthly spend
            0,     // depth
            &config,
        );

        // Cost would exceed daily ceiling
        match result {
            CostCheckResult::DailyLimitExceeded { .. } => {}
            _ => panic!("expected DailyLimitExceeded"),
        }
    }

    #[test]
    fn t6_depth_exceeds_max() {
        let config = CostConfig {
            daily_ceiling_usd: 10.0,
            monthly_ceiling_usd: 200.0,
            max_depth: 2,
        };

        let result = check_cost_and_depth(
            1.0,   // estimated cost (OK)
            5.0,   // current daily spend (OK)
            100.0, // current monthly spend (OK)
            3,     // depth exceeds max (2)
            &config,
        );

        // Depth check fails
        match result {
            CostCheckResult::DepthExceeded { depth, max } => {
                assert_eq!(depth, 3);
                assert_eq!(max, 2);
            }
            _ => panic!("expected DepthExceeded"),
        }
    }

    #[test]
    fn cost_ok_within_budget() {
        let config = CostConfig::default();

        let result = check_cost_and_depth(
            1.0,   // estimated cost
            5.0,   // current daily spend
            100.0, // current monthly spend
            0,     // depth
            &config,
        );

        assert_eq!(result, CostCheckResult::OK);
    }
}
