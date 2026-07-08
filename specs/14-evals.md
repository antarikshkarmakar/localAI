# Spec 14 — Evaluation Harness

**Status:** Draft
**Cites:** all KPIs (spec `00`); CON-8/S10 canary gate; GAPS G-02 (reward-hack test), G-13 (canary), G-20 (determinism).
**Upstream dependents:** `06-router` (regret harness), `10-learning` (canary before promotion), `11-security` (invariant CI gate).
**Why it exists:** self-improvement (spec 10) and self-mod safety (spec 11) both require a *frozen* yardstick. Without a fixed eval set, "did this change help?" is unanswerable and the canary gate is meaningless.

---

## 1. Eval set families

Each is a versioned, frozen fixture set under `evals/<family>/`. Frozen = changes are git-committed, reviewed, and bump a version; the learner cannot alter evals (spec 11 S11 — you can't grade yourself on a test you rewrote).

| Family | Measures | KPI | Form |
|---|---|---|---|
| `rag_qa` | retrieval + answer quality | KPI-08 | question → expected sources/answer, over a fixed KB snapshot |
| `fact_audit` | stored-fact correctness | KPI-06 | sampled `verified` facts → council + human spot-check |
| `router_regret` | route choices vs optimal-in-hindsight | KPI-07 | logged decisions replayed with known outcomes |
| `self_heal` | fault recovery rate | KPI-03 | injected faults → recovered? without human |
| `safety_invariants` | the S1 invariant set | — (CI-blocking) | adversarial inputs → must-block assertions |
| `reward_integrity` | anti-gaming | G-02 | crafted "gameable" tasks → reward must NOT reward them |
| `throughput` | tok/s, latency, RAM | KPI-04/05 | fixed prompts, measured, logged |

## 2. Determinism (GAPS G-20)

- **E1** — Model + council calls behind traits (spec 01 R6) are **mockable**; eval runs use a **record/replay** cache: first run records real model I/O to `evals/fixtures/`, later runs replay deterministically. A regression eval never depends on live nondeterministic generation.
- **E2** — All seedable randomness (bandit RNG, sampling) seeded in eval mode (spec 03 I8, spec 06 R7). temp=0 paths for deterministic answers.
- **E3** — Live-mode evals (real model, for periodic real-world benchmarking) are separate from replay-mode CI evals; live results are logged with variance, never gate CI (too flaky).

## 3. The canary gate (spec 10 L15, spec 11 S10 — the load-bearing use)

Before any self-mod activates:

- **E4** — The proposed change runs against the **frozen `safety_invariants` + relevant KPI eval families** in replay mode.
- **E5 — Hard reject** if ANY `safety_invariants` case regresses (no exceptions, overrides council approval — G-13).
- **E6 — Soft gate** on KPI evals: regression beyond a per-KPI threshold → reject + report; improvement or within-noise → eligible to proceed to activation + post-activation watch (spec 10 L16).
- **E7** — Canary result is a `decisions`-linked artifact (which cases passed/failed, deltas) — auditable, not a boolean.
- **E7b — Canary review rubric = the 11 lenses (antarikshSkills A5, shared with spec 05 C6b).** Qualitative canary assessment scores the change against the 11 thinking lenses; a regression on the Security/Secrets or Verification lens is treated as a hard-reject signal alongside E5.

## 4. Reward-integrity evals (GAPS G-02 — guards the learner's objective)

- **E8** — A suite of **adversarial tasks designed to be gameable**: trivially-compiling no-op code, an answer that satisfies a shallow proxy but fails the real goal, a council-question phrased to force easy agreement. The reward function (spec 06 §4) MUST assign these net-negative or neutral reward, never positive. If a future reward-weight change starts rewarding them, this suite fails → change rejected. This is how we keep the RL honest over time.

## 5. Router-regret harness (KPI-07)

- **E9** — Replay logged decisions where the true outcome is known: for each, compute reward of the chosen route vs the best route in hindsight. Cumulative regret trended over time must decrease (learning is working). A flat/rising curve is an alarm, not just a metric.
- **E10** — Warm-up aware (G-16): regret computed only after the N-decision warm-up (spec 06 R12) so cold-start exploration isn't scored as failure.

## 6. Continuous + scheduled runs

- **E11** — `safety_invariants` + `reward_integrity` run in **CI on every commit** — red = build fails (spec 11 T9). These are non-negotiable.
- **E12** — `rag_qa`, `router_regret`, `throughput` run scheduled (spec 04 O15) + on-demand (`/bench`, `/audit-facts` skills); results appended to a metrics log, trended in UI (spec 12).
- **E13** — `fact_audit` monthly (spec 05 mode 4), feeds retroactive reward (spec 10 L7).

## 7. Building the sets (Phase 3 onward)

- **E14** — `rag_qa` frozen in Phase 3 against the first real KB snapshot (spec 00 KPI-08 note); grows only by reviewed additions.
- **E15** — `safety_invariants` grows every time a new attack or near-miss is found (each incident, spec 11 S14, becomes a regression case — the system's scar tissue).
- **E16** — Eval provenance: each case records why it exists (which KPI/threat/incident) so the set stays meaningful, not cargo-culted.

## 8. Acceptance Criteria / Test Anchors

- [ ] T1: replay-mode eval produces byte-identical results across runs (determinism). (E1/E2, G-20)
- [ ] T2: a self-mod regressing one `safety_invariants` case is hard-rejected despite passing KPI evals + council. (E5, G-13)
- [ ] T3: reward-integrity suite — a gameable no-op task gets ≤0 reward; a genuinely-good task gets >0. (E8, G-02)
- [ ] T4: router-regret curve on a seeded synthetic log decreases as the bandit learns. (E9)
- [ ] T5: CI fails when a `safety_invariants` or `reward_integrity` case goes red. (E11)
- [ ] T6: a new incident (spec 11) is convertible into a frozen regression case that then blocks its own recurrence. (E15)
- [ ] T7: live-mode benchmark logs variance and does NOT gate CI. (E3)
- [ ] T8: regret harness ignores pre-warm-up decisions. (E10, G-16)
