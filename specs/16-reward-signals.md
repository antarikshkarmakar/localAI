# Spec 16 — Reward-Signal Capture

**Status:** Draft
**Cites:** KPI-07, KPI-09; GAPS G-02 (reward hacking); REVIEW RV-05 (this is the learning critical path).
**Upstream of:** `06-router` §4 (reward definition consumes these signals), `10-learning` §3 (attribution).
**Why it exists:** specs 06/10 define *what* reward means and *how it's attributed*, but nothing defined *how the raw signals are physically captured*. Without capture, `correction_penalty` is never populated and the bandit only ever sees cheap proxies (compiled/tests) — the exact reward-hacking surface G-02 warns against. This spec closes that hole.

---

## 1. Signal sources — concrete capture mechanisms

Every signal lands as a `rewards` row (spec 06 R10 schema) linked to the originating `OnRoute` decision event. Raw, separate, timestamped by `seq` not wall-clock (G-09).

| Signal | Captured how | Sign | Latency |
|---|---|---|---|
| `compiled` | worker exit code of build step | + / − | immediate |
| `tests_pass` | worker test-runner exit + parsed counts | + / − | immediate |
| `diff_applied` | change merged to main worktree (spec 08 A15) | + | at merge |
| `answer_accepted` | UI explicit accept (spec 12) OR implicit-accept timeout | + | seconds–hours |
| `answer_rejected` | UI explicit reject/thumbs-down | − | seconds |
| `reverted` | **git-hook** detects revert/amend/reset of an agent-authored commit within hold window (§3) | −− | hours–1 day |
| `file_reedited` | **file-watch** on agent-touched paths: human edits the same lines within hold window | − | hours |
| `council_agreed` | fact-check verdict (spec 05) — *weak* signal, never sole (G-02/G-04) | small + | minutes |
| `audit_confirmed` / `audit_failed` | monthly fact audit (spec 05 mode 4) | + / −− retroactive | ~monthly |
| `cost` / `latency` | ledger `cost_tokens`, timing | − (penalty) | immediate |
| `human_correction` | **explicit user-authored rule** ("Learned" entry, antarikshSkills A3) captured at rollup (spec 04 O15) | **strong, highest weight** | session end |

## 2. The four capture channels

- **RS0 — Human-correction channel (highest signal, adopted from antarikshSkills A3):** when the user explicitly corrects the *process* (not just this answer) — captured as a "Learned" rule at session-end rollup (spec 04 O15, antarikshSkills `/ak-compact` step 4) — this is the **strongest, most trustworthy reward signal**, weighted above all inferred signals. It names the failure explicitly rather than inferring it from a revert. Also spawns a procedural observation (spec 10 L9) if it generalizes. A named correction beats ten inferred ones.
- **RS1 — UI channel:** every answer carries a `trace_id` (spec 12 U3). UI exposes accept / reject / correct. **Implicit-accept rule:** no negative signal within the hold window AND the artifact wasn't reverted/re-edited → weak positive (silence ≈ acceptance, but weaker than an explicit thumbs-up). Explicit reject is a strong immediate negative.
- **RS2 — Git channel (the load-bearing one for code, RV-05):** agent commits are authored with a trailer `Localai-Job-Id: <id>` and a distinguishable author. A **git post-commit / post-rewrite hook** (installed in each worktree + the main repo) reports back to the Brain MCP server (spec 07 H14):
  - a `revert`/`reset`/`amend` touching an agent commit within the hold window → `reverted` (strong negative).
  - the agent commit surviving the hold window untouched, with later commits building on it → `diff_applied` confirmed positive.
- **RS3 — File-watch channel:** Brain watches paths an agent touched (from the diff, spec 08). A human editing those exact regions within the hold window → `file_reedited` (mild negative: the work needed rework). Distinguish from *extending* (new lines nearby = neutral/positive) vs *rewriting* (same lines changed = negative).

## 3. Hold window & booking (spec 06 R9)

- **RS4** — Computed reward is NOT booked at answer time. It's booked when the hold window closes (default 24h, ordered by `seq`/rowid to survive WSL clock drift, G-09).
- **RS5** — Any negative signal (reject / revert / re-edit) arriving inside the window flips the computed reward negative regardless of the cheap proxies. **This is the anti-gaming mechanism**: trivially-compiling useless code passes `compiled`+`tests_pass` but gets reverted/re-edited → net negative → bandit does not learn to produce it (G-02, spec 14 E8 reward-integrity eval guards this).
- **RS6** — Late signals (audit at ~monthly, spec 05 C9) apply *retroactive* reward via `rewards.superseded_by` (spec 06 R11) — the decision's posterior is revised even long after booking. The bandit unlearns routes that sourced facts which later failed audit.

## 4. Attribution rules

- **RS7 — Single decision, multiple signals:** all signals for one task link to the same `OnRoute` `decision_event`. The computed reward is a weighted combine (spec 06 §4 weights `w1..w5`).
- **RS8 — Credit assignment across a chain:** if a task escalated (LOCAL → COUNCIL → AGENT), reward attributes to *each* route decision in the chain proportionally — the route that *resolved* it gets the largest share; routes that failed and escalated get a small negative (they cost time/money without solving). Prevents "escalate everything" from looking free.
- **RS9 — No signal ≠ zero signal for learning:** tasks that never get a clear outcome (user walked away, ambiguous) are marked `unattributed` and **excluded** from bandit updates — not scored as failure. Prevents silence from poisoning the posterior (distinct from RS1 implicit-accept, which requires the artifact to have *survived*).

## 5. Anti-gaming guarantees (G-02, ties to spec 14)

- **RS10** — Proxy signals (`compiled`, `tests_pass`, `council_agreed`) can NEVER alone produce a strong positive reward. Strong positive requires a *durability* signal (survived hold window unreverted OR explicit human accept). Codified as an invariant tested by spec 14 E8.
- **RS11** — Reward weights are versioned config (spec 06 R10); because raw signals are stored separately (§1), any weight change recomputes history — and a weight change that would start rewarding the E8 gameable-task suite fails CI (spec 14 E11).

## 6. Acceptance Criteria / Test Anchors

- [ ] T1: agent commit reverted within hold window → git hook reports `reverted`; computed reward for that route goes net-negative despite `compiled`+`tests_pass` positive. (RS2/RS5, G-02)
- [ ] T2: answer with no negative signal + artifact survives hold window → weak positive booked (implicit accept). (RS1/RS4)
- [ ] T3: explicit UI reject → strong immediate negative, no need to wait for window. (RS1)
- [ ] T4: human re-edits the exact lines an agent wrote within window → `file_reedited` negative; editing *nearby* new lines → not penalized. (RS3)
- [ ] T5: escalated chain LOCAL→AGENT that succeeds at AGENT → AGENT gets main positive credit, failed LOCAL gets small negative. (RS8)
- [ ] T6: ambiguous/abandoned task → `unattributed`, excluded from bandit update (not scored as failure). (RS9)
- [ ] T7: monthly audit failure → retroactive negative applied to a decision booked weeks earlier. (RS6, spec 05 C9)
- [ ] T8: proxy-only signals cannot yield strong positive without a durability signal. (RS10, ties spec 14 E8)
