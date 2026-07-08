# Spec 05 — LLM Council

**Status:** Draft
**Cites:** OBJ-4 (trustworthy answers), CON-8 (self-mod gate), CON-11 (cost ceiling), CON-13 (secret filter) (spec `00`); KPI-06; GAPS G-04 (collusion), G-06 (cost), G-12 (outage), G-14 (secret leak).
**Downstream:** `06-router` (COUNCIL_* routes), `10-learning` (self-mod review, audits), `11-security`.

---

## 1. Purpose

The council is the Brain's external check — three cloud LLMs (Claude, OpenAI, Gemini) used **only when local confidence is insufficient or stakes are high** (router decides, spec `06`). It exists to make the autonomous learner *trustworthy*, not to answer everything. It is expensive (money + latency + privacy surface), so every call is gated, logged, and cost-attributed.

## 2. Adapter trait

```rust
// core crate
pub trait CouncilMember {
    fn id(&self) -> &str;                         // 'anthropic' | 'openai' | 'gemini'
    fn model(&self) -> &str;                      // from config, liveness-pinged at startup
    async fn ask(&self, prompt: Prompt, ctx: &CouncilCtx) -> Result<Verdict, MemberError>;
    fn cost_estimate(&self, prompt: &Prompt) -> Cost;
}
```

- **C1** — One adapter per provider; API keys from env only (CON-9). Prompts pass the OnEgress hook (spec `07` H9) → CON-13 secret redaction before leaving the machine. **Never send local secrets or full private files to the council** — send the minimal claim + evidence excerpt.
- **C2** — Model ids live in config, liveness-pinged at startup (GAPS G-12); a dead/renamed model fails loud at boot, not mid-decision.
- **C3** — Per-provider circuit breaker: consecutive failures / rate-limit → open breaker, back off, mark member `unavailable`. Breaker state is a `BrainStatus` field (UI-visible).

## 3. Modes

### Mode 1 — Decision (`COUNCIL_DECIDE`)
Judgment / security / irreversible questions.
- **C4** — Brain drafts a position → each *available* member independently critiques (blind to each other's answers — no shared thread, prevents anchoring) → synthesis by a **rotating chair** (chair role rotates per call so no single provider dominates framing) → verdict + **explicit dissent** recorded in `decisions`.
- **C5** — Dissent is preserved, never averaged away. A 2-agree/1-dissent decision stores the dissent and its reasoning; downstream (and the user) can see the split.

### Mode 2 — Security review (self-modification gate, CON-8)
Before any self-mod activates (prompt-library promotion, router weights, config).
- **C6** — Adversarial framing: members are asked to *find the failure*, not approve. Diff + intent + affected safety invariants sent. Any member flagging a safety regression → **block by default**, escalate to user. Not majority-vote — a single credible safety objection blocks (fail-safe, not fail-consensus).
- **C6b — Review rubric = the 11 lenses (adopted from antarikshSkills A5).** The security review evaluates the change against the 11 thinking lenses: Core Goal, Persona, Prerequisites, Context Bounds, Edge Cases, Portability, Token/Cache Efficiency, Error Handling, **Security/Secrets**, Verification Plan, Evolution Path. A concrete checklist beats open-ended "find problems" — same rubric is reused by the self-mod canary (spec 14 / spec 10 L10).

### Mode 3 — Fact-check (`COUNCIL_FACT`)
Claim + local evidence → verdict per member.
- **C7** — Each returns `supported | refuted | unverifiable` + confidence + **required citation**. For *empirical* claims, `supported` MUST include a resolvable primary/secondary source, not assertion (GAPS G-04). Reasoning-only agreement does not verify an empirical claim.
- **C8 — Storing a fact** (drives OKF `status`, spec `02` M7):
  - ≥2 `supported` **with** at least one resolvable source, no `refuted` → `verified`
  - any `refuted`, or split → `disputed` (all verdicts stored in the OKF body)
  - <2 members available → `unverifiable` (never fabricate consensus from one voice, G-12)

### Mode 4 — Calibration audit (KPI-06)
Periodic (monthly) sampling of stored `verified` facts + a sample of the Brain's own past answers, re-checked by the council.
- **C9** — Produces a calibration score (fraction confirmed) trended over time. Facts that fail re-check flip to `disputed`/`superseded` AND trigger **retroactive negative reward** on the router decision that produced them (spec `06` R11 — the Brain unlearns the route that sourced a bad fact).

## 4. Anti-collusion rules (GAPS G-04)

The three providers are **not** independent — overlapping training data, shared popular misconceptions. Consensus is weaker evidence than it looks.

- **C10** — Council never marks `verified` on *reasoning alone* for empirical claims (C7). Source required.
- **C11** — Track **per-domain council accuracy** over time (audit outcomes by `domain`). Domains where the council is jointly unreliable get a discount factor applied to their consensus weight, surfaced to the router as lower confidence.
- **C12** — Unanimous agreement on a *contested/niche* claim raises a flag, not certainty — logged with a `low_diversity` marker for later audit.
- **C13** — Where feasible, one member is prompted for the *steelman of the opposing view* (contrastive check, kept from draft's counterfactual idea) so at least one voice actively probes for the failure.

## 5. Cost governance (CON-11, GAPS G-06)

- **C14** — Every council call cost-estimated pre-flight (`cost_estimate`) and recorded post-flight in `events.cost_tokens` + `cost_usd`.
- **C15** — Daily + monthly $ ceilings in config. Cumulative spend checked before each call; ceiling reached → council routes **unavailable**, router degrades to LOCAL/SEARCH + notifies user (spec `06` R3).
- **C16** — Recursion guard: a council call triggered inside a self-heal chain carries the chain depth; depth > 2 → refused (prevents heal→council→heal→council loops, GAPS G-07).
- **C17** — Cheapest-sufficient tiering: fact-check may use each provider's cheaper/faster tier; only Decision/Security-review use top models. Configurable per mode.

## 6. Decisions table

```sql
CREATE TABLE decisions (
    id INTEGER PRIMARY KEY,
    event_id INTEGER NOT NULL REFERENCES events(id),  -- the COUNCIL_* route event
    mode TEXT NOT NULL,                -- 'decide'|'security'|'fact'|'audit'
    question TEXT NOT NULL,
    chair TEXT,                        -- which provider synthesized (rotating, C4)
    verdict TEXT NOT NULL,             -- synthesized outcome
    votes_json TEXT NOT NULL,          -- per-member: stance, confidence, citation, dissent
    diversity_flag TEXT,               -- 'low_diversity' etc (C12)
    cost_usd REAL,
    created TEXT NOT NULL
);
```

## 7. Acceptance Criteria / Test Anchors

- [ ] T1: Empirical claim with 3× `supported` but ZERO resolvable sources → NOT marked `verified` (stays `unverifiable`/`disputed`). (C7, C10, G-04)
- [ ] T2: Security review — one member flags a safety regression, two approve → change BLOCKED, user escalated. (C6, fail-safe)
- [ ] T3: One provider `unavailable` mid-fact-check → result `unverifiable`, no fabricated 1-voice consensus. (C8, G-12)
- [ ] T4: Monthly audit flips a previously-`verified` fact to `disputed` → OKF status updated AND retroactive negative reward booked on the sourcing route. (C9, links spec 06 R11)
- [ ] T5: Cost ceiling reached → next COUNCIL_* route unavailable; LOCAL still served; user notified. (C15, CON-11)
- [ ] T6: Self-heal chain at depth 3 requesting council → refused. (C16, G-07)
- [ ] T7: Secret accidentally in an evidence excerpt → redacted by OnEgress before any provider call. (C1, G-14)
- [ ] T8: Chair rotates across successive Decision calls (no provider chairs twice running). (C4)
- [ ] T9: Dissent in a 2-1 decision is stored verbatim in `votes_json`, not discarded. (C5)
