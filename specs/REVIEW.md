# REVIEW — Fable's adversarial self-review of specs 00–14

**Reviewer:** Claude Fable 5 (same author as the specs — deliberate red-team of my own work)
**Date:** 2026-07-07
**Verdict:** Architecture is sound and unusually safety-complete. The real risks are **not technical correctness** — they're **scope, latency, cost, and data-starvation of the learning loops on single-user volume.** Below, ranked by how likely each is to actually hurt you.

Severity: **R1** = likely to kill/stall the project · **R2** = will bite hard, needs a decision · **R3** = polish / watch.

---

## R1 — Likely to stall the project

### RV-01 — Scope vs. one developer, one machine. No walking skeleton.
13-crate workspace, 15 subsystems, council + bandit + self-heal + agent orchestration. This is a multi-person-year system. The phase plan is **bottom-up** (infra → features) — meaning nothing is *usable* until ~Phase 5. Solo projects that don't produce something they use early tend to die.
**Fix — define a thin vertical slice (Phase 1.5, "walking skeleton") that you actually use daily:**
> local chat (spec 03) + RAG over your OWN notes (spec 02, hand-loaded, no scraper) + activity ledger (02) + minimal UI chat panel (12). One escalation: a manual "ask council" button (05, no router).
No workers, no bandit, no self-heal, no agents. If that slice is genuinely useful to you, the rest earns its place. If it isn't, you learned that cheaply. **Add this as Phase 1.5 in PLAN.md before Phase 2.**

### RV-02 — The learning loops are data-starved on single-user volume.
Contextual bandit (06) and prompt A/B (10 L9) need *samples* to learn. A single user generates maybe tens of decisions per task-class per month. You will **never reach statistical significance** on prompt A/B; the bandit posterior will be dominated by priors for a very long time. Building elaborate RL machinery that can't get signal is wasted effort.
**Fix — reframe honestly:** for the foreseeable future, **hand-set priors + heuristics do 95% of the routing** (spec 06 R12 already provides them). The bandit is a *slow-burn refinement*, not a core feature. Prompt "evolution" (10 §4) is realistically **manual curation with stats as a guide**, not autonomous A/B. Down-scope Phase 7/10 accordingly; don't gate value on learning that can't converge. KPI-07 (regret decreasing) may be flat for months — that's expected, not failure.

### RV-03 — Latency reality is brutal and unstated.
6 tok/s (KPI-04) → a 500-token answer = **~83 seconds**. Self-consistency k=3 = 3×. The self-heal repair ladder (09 §3) runs multiple full generations per iteration → **many minutes per repair attempt**. The system will feel glacial for anything non-trivial, and the "autonomous background researcher" competes with interactive chat for the *same single-generation queue* (03 I1).
**Fix:** (a) State the latency budget honestly in spec 00 as an NFR — set user expectation. (b) Use the **E4B fast model** (ADR-003) aggressively for classification, self-consistency, and background distill; reserve the 12B for final answers. (c) Reconsider k=3 self-consistency as default — it triples the slowest operation. Make it opt-in for high-stakes only. (d) Background compounding effectively **pauses during interactive use** (shared queue + MemoryGuard soft watermark) — say so; it's a nights-and-weekends learner, not concurrent.

---

## R2 — Will bite hard; decide now

### RV-04 — 22 GB budget is thinner than the specs admit.
Gemma 4 12B Q4 (~8) + KV@32K (~3) + MTP drafter (~2) = **~13 GB model alone**. Brain+embeddings ~3. That leaves ~4.5 GB for "3 workers" — but a single headless-chrome scrape (spec 13 D7) can eat 1.5–2 GB by itself, and the E4B fast model if co-resident is another ~3 GB. **You cannot run the 12B + E4B + 3 heavy workers concurrently.** The spec 01 R11 table is optimistic.
**Fix:** make it explicit — **model OR heavy workers, not both at full tilt.** MemoryGuard already sheds load (01 R13), but the *consequence* (background work stalls whenever the big model is hot) must be a stated design property, not a surprise. Consider: default to E4B resident, load 12B on-demand for hard queries, swap out after (03 I5 hot-swap makes this viable). Revisit the R11 table with these numbers.

### RV-05 — Reward-signal capture is undefined — and it's the critical path.
The entire learning loop (06/10) hinges on knowing "did the user accept / revert / correct?" (06 R8, R9). **No spec defines HOW that signal is captured.** Git-watch for reverts? Explicit thumbs in UI? Detecting a re-edit of a file the agent touched? Without this, `correction_penalty` is never populated and the bandit only ever sees the cheap proxies (compiled/tests) — exactly the reward-hacking surface G-02 warns about.
**Fix:** add a **reward-signal spec** (extend 06 or a new §): define concrete capture — (a) UI accept/reject on answers, (b) git hook detecting revert/amend of agent-authored commits within the hold window, (c) file-watch on agent-touched paths. This is load-bearing; it belongs in an early phase, not deferred.

### RV-06 — Council escalation IS an egress of your private context (OBJ-1 tension).
Every council call ships your query + evidence to three separate clouds (Anthropic, OpenAI, Google). That's the *opposite* of data sovereignty for those queries. The SecretFilter (11 S5) strips keys, but not the *substance* of what you're thinking about. This is an inherent trade-off, currently soft-pedaled.
**Fix:** make it an explicit, visible privacy decision. (a) Spec 12 should surface "this will send X to 3 clouds — proceed?" for council routes, or a per-domain policy ("never escalate anything tagged `personal`/`work-confidential`"). (b) Consider a "sovereign mode" toggle: local-only, council disabled, for sensitive sessions. (c) State in spec 00 that OBJ-1 holds for *local* operation; escalation is an explicit, logged privacy exception.

### RV-07 — Backups are assumed but never created.
Spec 09 H8 says "restore from last good backup" — but **no spec creates backups.** Single SQLite file + `kb/` tree = single point of failure. Corruption or a bad migration with no backup = total memory loss (the thing OBJ-3 spent months compounding).
**Fix:** (a) **Put `kb/` under git** — free versioning, history, diff-based reconciliation (spec 09 H5 gets easier), trivial revert. This is a big, cheap win and should be a stated invariant. (b) Scheduled SQLite backup (`VACUUM INTO` / litestream-style) as a maintenance job (spec 04 O15). (c) Since `kb/` is ground truth (02 M1) and git-tracked, the DB is fully rebuildable — so the real backup burden is just the ledger + procedural tables.

### RV-08 — TDD can't cover the qualitative core; say so.
Global CLAUDE.md mandates test-first. But the *value-defining* behaviors — distillation quality, RAG relevance, fact-check accuracy, answer usefulness — **cannot be unit-tested against a mock.** They're eval-driven (spec 14), inherently fuzzy, need a real model. Pretending these are TDD-able will produce either fake tests or paralysis.
**Fix:** split the workflow explicitly. **Deterministic plumbing (queue, ledger, provenance gate, budget guard, schema) → strict TDD** (these have the T1..Tn anchors, good). **Qualitative behaviors → eval-driven** (spec 14 sets, scored, tracked, not pass/fail-gated in CI). State which spec anchors are TDD vs eval so you don't try to mock the unmockable.

---

## R3 — Polish / watch

- **RV-09 — systemd in WSL2 is opt-in.** The watchdog (01/09 H9) assumes systemd; WSL2 needs `systemd=true` in `wsl.conf` (recent WSL only). Fallback: a lightweight init/`supervisor` script or a Windows Task-Scheduler-launched WSL heartbeat. Note it in the runbook; don't assume.
- **RV-10 — 384-d MiniLM is weak for code + long technical docs.** Fine for prose notes; mediocre for code retrieval (a core use case). Consider a code-aware embedding for the code domain, or accept the limitation and lean on tree-sitter structural retrieval (13 D9) for code instead of vectors. Flag in ADR-002 / spec 02.
- **RV-11 — Contradiction check: audio path.** spec 00 §5 (whisper.cpp "may be droppable"), ADR-003 (drop pending Phase 5), spec 13 D12/D13 (native primary, whisper fallback). These are *consistent* now but spread across 3 files — when Phase 5 resolves it, update all three in one commit or they'll drift (see the tracker-drift memory).
- **RV-12 — "verified" fact confidence is a council artifact, and council has blind spots (G-04).** The confidence number in OKF frontmatter (02) risks false precision. Keep it, but treat `verified` as "council + source agreed", NOT "true". The audit loop (05 mode 4) is what keeps it honest — make sure it actually runs (it's easy to skip a monthly job).
- **RV-13 — No offline/degraded council story for fact-check-heavy cold start.** Early on, everything wants verification but council cost + latency throttles it. Most early facts will sit at `draft` forever. **That's fine** — but state it: `draft` is the normal resting state; `verified` is earned by *use* (a fact retrieved often enough gets prioritized for verification), not by verifying everything eagerly.

---

## What holds up well (don't second-guess these)
- Provenance gate (07/11) — the single best decision; injection defense is architectural, not bolted-on.
- Multi-signal delayed reward (06 §4) — correct even if data-starved; the *definition* is right.
- OKF-as-ground-truth + rebuildable DB (02 M1) — resilient, and pairs perfectly with the git recommendation (RV-07).
- Single-writer Brain + durable queue + write-ahead intent (01/04) — crash-safety is genuinely well-thought-out.
- Immutable safety set (11 S11) — the learner can't touch its own guardrails. Keep this inviolable.
- The self-correcting loop (audit → retroactive reward → unlearn) — elegant; it's the actual "self-improving" mechanism.

## Recommended changes to PLAN.md (actionable)
1. **Insert Phase 1.5 "walking skeleton"** (RV-01) — usable daily driver before building infra depth.
2. **Add a reward-signal capture spec** (RV-05) to an early phase — it's the learning critical path.
3. **Put `kb/` under git + add a backup maintenance job** (RV-07) — cheap, high-value, do it in Phase 1.
4. **Re-cost the memory table** (RV-04) with real E4B/12B/worker numbers; adopt "E4B resident, 12B on-demand".
5. **Downgrade learning-loop expectations** (RV-02) — priors/heuristics are the product for months; RL is a slow-burn.
6. **Add latency + privacy-of-escalation NFRs to spec 00** (RV-03, RV-06) — set expectations honestly.
