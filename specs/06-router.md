# Spec 06 — Escalation Router & Decision Learning

**Status:** Draft
**Cites:** OBJ-1, OBJ-2 (spec `00`); KPI-01, KPI-07; GAPS G-02 (reward hacking), G-16 (cold start), G-09 (timing).
**Downstream:** `05-council`, `10-learning`, `14-evals`.
**Why load-bearing:** the reward definition (§4) must be fixed BEFORE the bandit is written, or the system learns the wrong objective and every later phase inherits it (GAPS G-02).

---

## 1. Router job

Given an incoming query + context, choose one route, cheapest-that-works:

```
route ∈ {
  LOCAL,            // local model + KB answer
  LOCAL_SELFCHECK,  // local model, k-sample self-consistency (harder, still local)
  SEARCH,           // web_search → scraper → distill → answer with citations
  COUNCIL_DECIDE,   // judgment/security/irreversible → council decision mode
  COUNCIL_FACT,     // conflict / high-stakes fact → council fact-check
  AGENT,            // delegate to CLI coding agent (spec 08)
}
```

- **R1** — Route is chosen by a **contextual bandit** over features (§3), NOT a fixed rule tree. Rules provide only the *priors* (§5, G-16). The bandit learns, per task-class, which route actually pays off.
- **R1b — LOCAL_SELFCHECK = Parallel-Distill-Refine, not majority vote (arXiv 2510.01123):** k drafts (serial on CPU anyway, I1 — same cost) → **distill** into a bounded workspace (E4B, cheap) → **refine** conditioned on the workspace (12B). Beats k-sample voting at matched compute, and the bounded workspace keeps the refine context small (RV-03, M3b spirit) instead of dragging k full drafts along. k and the workspace budget are config. The sequential variant (k=1, iterate) is the same machinery the repair ladder already applies to failure evidence.
- **R2** — Every route decision emits an `OnRoute` hook + ledger event with: features, chosen route, prior, exploration flag, `trace_id`. This is the reward-attribution anchor.

## 2. Confidence signals (features feeding the router)

| Signal | Source | Meaning |
|---|---|---|
| `kb_score` | retrieval auditor 1–5 confidence grading (agentic-rag pattern) + top-1/top-5 fusion (spec `02` M11) | do we have relevant knowledge, graded |
| `self_consistency` | k=3 cheap local samples agree? (temp>0) | model's own stability on this query |
| `task_class` | classifier: {code, math, factual, judgment, humanities, ops} | which competency |
| `stakes_class` | {trivial, normal, irreversible, security, external-facing} | cost of being wrong |
| `provenance_mix` | fraction of in-context chunks that are Untrusted/Unverified (spec `07`) | injection/uncertainty risk |
| `est_cost` | token/$ estimate per route | budget awareness (CON-11) |
| `freshness_need` | query asks for recent/changing info? | forces SEARCH regardless of KB |

- **R3** — Hard overrides (bandit cannot veto these):
  - `stakes_class ∈ {irreversible, security}` → minimum route `COUNCIL_DECIDE` (never silent-local on high stakes).
  - `provenance_mix > 0` AND route would use a Privileged tool → blocked (defers to spec `07` H4).
  - `freshness_need = true` AND `kb_score` stale → `SEARCH` floor.
  - cost circuit-breaker tripped (CON-11) → cloud routes unavailable, degrade to LOCAL + notify.

## 3. Bandit design

- **R4** — Algorithm: **Thompson sampling** over a linear/logistic contextual model per route (no GPU, cheap, natural exploration). Choice recorded in ADR-005; LinUCB is the fallback if Thompson variance is unstable on sparse early data.
- **R5** — Context = feature vector (§2, discretized/normalized). Arms = routes. Reward = §4. Posterior updated on reward arrival (delayed, §4).
- **R6** — Exploration is **bounded + safe**: never explores into a *cheaper/riskier* route on high-stakes queries (exploration only among routes ≥ the prior floor). Exploration rate decays as per-class evidence accumulates.
- **R7 — RNG is seeded** (GAPS G-20) so bandit behavior is reproducible in tests; production seed rotates but is logged per session.

## 4. Reward definition (GAPS G-02 — the anti-gaming core)

Reward is **multi-signal, delayed, and revert-aware**. No single proxy is ever the reward.

```
reward(route, task) = w1·correctness
                    + w2·efficiency          (cheaper route that still worked = bonus)
                    - w3·cost                (tokens + $)
                    - w4·latency
                    - w5·correction_penalty  (user edited/rejected/reverted)
```

- **R8 — Correctness is composite, task-specific, and NOT self-reported:**
  - code: `compiled AND tests_pass AND diff_applied AND NOT reverted_within_hold_window`
  - factual: council fact-check `supported` OR user confirmed OR citation resolves to primary source
  - judgment: user accepted the recommendation (explicit or by acting on it)
  - Never "model said it was confident." Never "council agreed" alone for empirical claims (G-04).
- **R9 — Delayed attribution / hold-back window:** reward is NOT booked at answer time. It's booked after a stabilization window (default 24h of wall-clock, but ordered by `seq`/rowid not wall-clock alone — G-09). A revert inside the window flips the reward negative. This kills "trivially-compiling useless code" reward hacking (G-02).
- **R10 — Raw signals stored separately** from the computed reward (`rewards` table: one row per signal + one computed). If weights (`w1..w5`) change later, rewards are recomputable — and mislabeling is auditable. Weights live in config, versioned (self-mod → CON-8 council gate).
- **R11 — Negative reward on discovered harm:** if a route's answer is later found wrong in a fact audit (spec `05` mode 4) or caused a revert/incident, retroactive negative reward is applied to that historical decision. The bandit *unlearns* bad routes.

```sql
CREATE TABLE rewards (
    id INTEGER PRIMARY KEY,
    decision_event INTEGER NOT NULL REFERENCES events(id),  -- the OnRoute event
    signal TEXT NOT NULL,          -- 'correctness'|'cost'|'latency'|'correction'|'computed'
    value REAL NOT NULL,
    booked_at TEXT,                -- null until hold window closes
    superseded_by INTEGER          -- if retroactively revised (R11)
);
```

## 5. Cold-start priors (GAPS G-16)

Day 1: no history, empty KB. Bandit seeded with hand-set priors so behavior is sane immediately:

| task_class | prior route |
|---|---|
| factual (fresh) | SEARCH |
| factual (static) | LOCAL then verify |
| code | LOCAL_SELFCHECK, AGENT if large |
| math | LOCAL_SELFCHECK |
| judgment / security | COUNCIL_DECIDE |
| humanities | LOCAL_SELFCHECK + COUNCIL_FACT on claims |

- **R12** — Priors are optimistic-but-bounded (encourage some early exploration without wild misroutes). KPI-01/KPI-07 are only *reported* after a warm-up threshold (N=500 decisions) so cold-start noise doesn't pollute metrics (spec `00` baseline-first rule).
- **R13** — Priors themselves are versioned config; as evidence accumulates the learned posterior overrides them, but priors remain the fallback when a novel task-class appears.

## 6. Acceptance Criteria / Test Anchors

- [ ] T1: High-stakes (`security`) query never routes below COUNCIL_DECIDE, even if bandit posterior favors LOCAL. (R3)
- [ ] T2: Reward-hacking sim — an agent that emits trivially-compiling no-op code gets NET NEGATIVE reward after the revert/hold window; bandit does not increase preference for that route. (G-02, R8/R9)
- [ ] T3: Retroactive negative reward — a fact later failing audit flips its original decision's reward and shifts the posterior. (R11)
- [ ] T4: Seeded RNG → identical route sequence across two runs on same inputs. (R7, G-20)
- [ ] T5: Cost circuit-breaker tripped → COUNCIL/SEARCH/AGENT routes removed from arm set; LOCAL still served. (R3, CON-11)
- [ ] T6: Cold start (empty tables) → factual-fresh query routes SEARCH via prior, not random. (R12, G-16)
- [ ] T7: Weight change in config → historical rewards recompute from stored raw signals without re-running tasks. (R10)
- [ ] T8: `provenance_mix > 0` + would-be-Privileged route → blocked, defers to harness. (R3, spec `07` H4)
