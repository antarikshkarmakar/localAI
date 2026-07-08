# ADR-005 — Router decision-learning algorithm

**Status:** Accepted
**Date:** 2026-07-06
**Cites:** KPI-07 (router learning); spec 06 §3, GAPS G-02, G-16.

## Context
The router (spec 06) must learn, per task-class, which route (LOCAL / SEARCH / COUNCIL / AGENT) pays off. Options: fixed rules, ε-greedy bandit, **Thompson sampling** (Bayesian), LinUCB (upper-confidence).

## Decision
**Contextual Thompson sampling** over a per-route linear/logistic reward model. Rules provide **priors only** (spec 06 R12), not the decision. LinUCB is the documented fallback if Thompson's posterior variance is unstable on sparse early data.

## Rationale
- **No GPU, cheap:** linear/logistic posteriors update in microseconds; fits the CPU budget.
- **Natural exploration:** Thompson's posterior sampling explores in proportion to uncertainty — no hand-tuned ε schedule, and exploration auto-decays as evidence accumulates (good for cold-start, G-16).
- **Bounded-safe exploration:** exploration is constrained to routes ≥ the prior's stakes floor (spec 06 R6) — never explores into a cheaper/riskier route on high-stakes queries.
- **Reward is the hard part, not the algorithm:** the anti-gaming work (multi-signal, delayed, revert-aware — spec 06 §4, G-02) matters far more than bandit choice. Thompson is a simple, well-understood substrate that lets the reward definition do the heavy lifting.

## Consequences
- Reward must be defined *before* the bandit is trained (spec 06 §4 fixed first) or it learns the wrong objective.
- Seeded RNG for reproducible tests (spec 06 R7, G-20).
- Per-route reward models are small + inspectable — feeds the Explain view (spec 12 U3) and regret harness (spec 14 E9).
- Re-open if task-classes explode in number (linear models per class may need pooling/hierarchical priors).
