# Spec 08 — CLI Coding-Agent Orchestration

**Status:** Draft
**Cites:** OBJ-2 (cost), OBJ-5 (resilience), CON-10 (untrusted), CON-11 (cost/depth) (spec `00`); KPI-09 (delegation yield); GAPS G-07 (fork bomb), G-17 (handoff poison).
**Downstream / depends on:** `04-orchestration` (job queue, caps), `07-harness` (provenance, MCP server), `11-security` (sandbox, review gate), `05-council` (risk review).

---

## 1. Purpose

The Brain delegates heavy coding to external CLI agents (`claude`, `codex`, `opencode`) rather than doing everything with the local 12B. Each delegation is a `jobs` row of kind `agent`, run by the `agent-run` worker (spec 04), isolated in a git worktree, driven by a structured **brief**, and closed by a structured **handoff**. Context survives across agents; artifacts are archived and searchable.

## 2. Agent adapter

```rust
pub trait CliAgent {
    fn id(&self) -> &str;                 // 'claude'|'codex'|'opencode'
    fn invoke(&self, ws: &Worktree, brief: &Path, ctx: &AgentCtx) -> AgentInvocation; // builds argv, env
    fn capabilities(&self) -> AgentCaps;  // strengths, cost tier, auth kind, max_context
    fn parse_result(&self, out: &Output) -> AgentOutcome;  // exit code + handoff → structured
}
```

- **A1** — One adapter per CLI, capability-tagged (cost tier, strengths e.g. refactor/tests/debug, auth kind, context limit). Adapters are the only place CLI-specific argv/flags live.
- **A2 — Selection:** router (spec 06, AGENT route) picks the agent by capability match + cost (CON-11) + past success stats (`agent_runs`). Cheapest-capable first; escalate on failure (spec 09 ladder).
- **A3 — Non-interactive invocation only** (`claude -p`, `codex exec`, `opencode run`); no TTY agent left waiting. Timeout per spec 04 O10.

## 3. Worktree lifecycle

- **A4** — Each agent job gets `git worktree add artifacts/<job-id>/ws <base-ref>` — isolated working copy on a throwaway branch. Parallel agents never collide (up to semaphore=3, spec 04).
- **A5 — Scrubbed environment (spec 11 S6):** worktree process env carries NO host API keys; the agent's own auth (its config/keychain) is its concern; Brain passes only scoped, short-lived tokens if a tool needs them.
- **A6 — Resource cap (spec 04 O7, G-07):** worktree process runs under cgroup/ulimit (mem + process count) so a runaway agent can't fork-bomb or exhaust RAM. Spawn depth inherited (≤2).
- **A7 — Teardown:** on success-merged or discard, `git worktree remove`; artifacts (below) are copied out first. An unchanged/empty worktree is auto-removed.

## 4. Brief format (`BRIEF.md` written into the worktree)

```markdown
---
job_id: <id>
base_ref: <sha>
agent: claude
depth: 1
mode: sync                  # sync | async | ongoing (CAO pattern: sync=single reply, async=task queued, ongoing=streaming updates)
budget_usd: 2.00
risk_tags: [auth]           # empty unless touching sensitive surfaces
success_criteria:           # machine-checkable where possible
  - tests_pass: "cargo test -p foo"
  - no_new_clippy_warnings: true
constraints:
  - "do not modify egress allowlist or SecretFilter"   # spec 11 S11
  - "stay within crates/foo; do not touch crates/core"
  - "keep changes atomic: ≤3 files, ≤100 lines/commit" # ai-auto-work pattern
---
## Goal
<one-paragraph objective, generated from the task + router context>

## Relevant knowledge (VERIFIED only)
<RAG chunks, spec 02 — status=verified ONLY; unverified/untrusted NOT injected as fact>

## Prior handoff (if continuation) — UNTRUSTED until verified
<prior agent's HANDOFF.md claims, each labeled verified✓ / claimed? (spec 07 H6)>
```

- **A8 — Brief context hygiene (G-17):** only `verified` KB goes in as fact. A prior agent's handoff enters as **untrusted** (spec 07 H6): claims cross-checked against actual worktree state (file exists? test named that?) and labeled `verified✓` or `claimed?`. The agent is told which is which. No blind trust chain.
- **A9 — Constraints include the immutable safety set** (spec 11 S11) explicitly, so even a well-meaning agent won't touch the guardrails.

## 5. Handoff format (`HANDOFF.md` — agent MUST produce)

```markdown
---
job_id: <id>
status: done | partial | blocked
tests: {ran: true, passed: 42, failed: 0}
diff_stat: {files: 3, +120, -14}
cost_usd: 1.40
---
## Done
## Failed / blocked (with reasons)
## Decisions made (and why)
## Claims about the codebase   # ← verified against ground truth on ingest (G-17)
## Next steps
```

- **A10 — Handoff is parsed back into the ledger + episodic memory** (spec 02), and its "Claims" section is **verified against the real worktree** before any claim is trusted downstream (G-17, spec 07 H6). Unverifiable claims are stored as `claimed`, not fact.
- **A10b — Verify-before-done (Boris B4, spec 04 O12b):** an agent run cannot be marked `done` unless the handoff shows its success criteria met (tests ran + passed per the BRIEF). `tests.ran=false` or unmet criteria → `partial`, not `done`. No reward for unverified agent work (spec 16). Uses antarikshSkills' handoff write→read→delete continuity (the parsed handoff, once ingested, is cleared from the worktree).
- **A11 — Missing/malformed handoff** → job `partial`, worker synthesizes a minimal handoff from the diff + logs, flags low-confidence. An agent that produces changes but no handoff still yields a usable record.

## 6. Artifacts

- **A12 — Every run archives** under `artifacts/<job-id>/`: `BRIEF.md`, `HANDOFF.md`, the diff (`patch`), full agent stdout/stderr log, test output, cost record. Indexed in `agent_runs`:

```sql
CREATE TABLE agent_runs (
    id INTEGER PRIMARY KEY, job_id INTEGER NOT NULL REFERENCES jobs(id),
    agent TEXT NOT NULL, base_ref TEXT NOT NULL, branch TEXT,
    status TEXT NOT NULL, tests_passed INTEGER, tests_failed INTEGER,
    diff_files INTEGER, diff_add INTEGER, diff_del INTEGER,
    cost_usd REAL, merged INTEGER DEFAULT 0,     -- KPI-09
    artifact_dir TEXT NOT NULL, created TEXT NOT NULL, finished TEXT
);
```

- **A13 — Artifacts are searchable** via the KB (handoffs + decisions embedded, spec 02) so future briefs can retrieve "how did we solve X before" (procedural memory, spec 10).
- **A14 — Retention (G-19, CON-12):** verbose logs pruned/compressed after N days; `BRIEF.md`, `HANDOFF.md`, and the diff are kept long-term (small, high-value).

## 7. Review & merge gate (spec 11 S13)

- **A15 — No auto-merge.** Agent diff → local review (local model + lint/test) → if `risk_tags` non-empty (auth/secrets/egress/CI) → **council security review** (spec 05 C6) → only then eligible to merge to the main worktree, and only with the configured confirmation (spec 11 S3).
- **A16 — Reward attribution (spec 06 R8):** merge + tests-pass + not-reverted-in-window = positive reward for the AGENT route and the chosen agent; revert/reject = negative. Feeds agent selection (A2) and router learning.
- **A17 — KPI-09:** `merged OR explicitly-useful` / total runs ≥ 70%. A run that produces a rejected diff still counts as useful if it surfaced a real blocker (recorded in handoff).

## 8. Acceptance Criteria / Test Anchors

- [ ] T1: Two agent jobs run concurrently in separate worktrees on separate branches — zero file collision, zero shared-state bleed. (A4)
- [ ] T2: Agent process env contains no host API key (asserted); scoped token only if declared. (A5, spec 11 S6)
- [ ] T3: Prior handoff claims "src/x.rs exists" but it doesn't → brief labels it `claimed?`, no downstream tool treats it as fact. (A8/A10, G-17)
- [ ] T4: Agent produces diff but no HANDOFF.md → worker synthesizes minimal handoff from diff; job `partial`, run still recorded. (A11)
- [ ] T5: Risk-tagged (`auth`) diff cannot merge without passing council security review. (A15, spec 11 S13)
- [ ] T6: Runaway agent hitting mem/process cap → contained by cgroup, job failed, host unharmed. (A6, G-07)
- [ ] T7: Depth-2 agent requesting to spawn another agent → refused (spec 04 O6). (A6)
- [ ] T8: Merged+passing+unreverted agent run → positive reward booked; a reverted one → negative. (A16, spec 06)
- [ ] T9: Old run's verbose log pruned after retention window; BRIEF/HANDOFF/diff retained + still KB-searchable. (A14/A13)
