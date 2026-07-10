# Spec 17 — Loop 4 Training Pipeline (cloud fine-tune of the fast model)

**Status:** Draft — contract defined now; implementation is **Phase 9+**. Nothing in Phases 1–8 depends on this spec except the capture contract it consumes (spec 16 RS12–15), which is built early precisely so this pipeline has years of data when it arrives.
**Cites:** OBJ-2 (cost), OBJ-3 (compounding), CON-7 (egress), CON-8 (self-mod gate), CON-13 (secrets) (spec `00`); GAPS G-01 (poisoned data), G-02 (reward hacking), G-13 (self-mod).
**Depends on:** `16-reward-signals` (trajectories), `03-inference` (registry, hot-swap), `14-evals` (canary), `11-security` (S10 chain, SecretFilter), `05-council` (review), `04-orchestration` (job kinds).
**Method candidates & trade-offs:** [docs/open-questions.md](../docs/open-questions.md) §"Loop 4 fine-tuning method" — KTO / DPO / RFT / council-distillation / GRPO-via-ART. Method chosen at Phase 9 by control-arm comparison, not now.

---

## 1. Scope & hard boundaries

- **P1 — Target model:** the **fast model only** (Gemma 4 E4B class, ADR-003). The 12B primary is NEVER fine-tuned — too load-bearing, too expensive to canary properly, and harness evolution (spec 10) is the 12B's improvement channel. If ADR-003 re-verification swaps the fast model family, this spec's target follows it.
- **P2 — Training location:** **cloud GPU, rented per campaign** (32 GB CPU box cannot train, spec 00). Campaign = bounded job: rent → train → return artifact → release. No standing training infra.
- **P3 — Never autonomous (CON-8, G-13):** a training campaign is *proposed* by the system (§3) but **initiated only by explicit human approval**, and the resulting model activates only through the full self-mod chain (§7). The learner cannot train itself into production.
- **P4 — Weights are versioned artifacts,** not mutable state: every campaign produces an immutable, hash-identified model artifact with full provenance (§6). Rollback = re-registering the prior artifact.

## 2. Dataset build — from RS14 views to training files

The dataset builder is a `jobs` row of kind `dataset_build`. Input: the trajectory store (spec 16 RS12). Output: a **dataset snapshot** — immutable files + manifest, hash-identified.

### 2.1 Export views → formats

| View (RS14) | Eligibility filter | Output format |
|---|---|---|
| **KTO** | booked reward, `durability_outcome ∈ {survived, accepted_explicit}` → label `true`; `{reverted, rejected}` → label `false`; `unattributed`/`reedited` excluded | `{prompt, completion, label: bool}` JSONL |
| **Distillation** | route chain ended in COUNCIL_*, council verdict accepted, fact-audit not-failed at build time | `{prompt, completion}` JSONL (completion = council answer) |
| **RFT** | `verifier.passed = true` (objective verifier only — tests/compile/citation, spec 06 R8; never council-agreement alone, G-04) | `{prompt, completion}` JSONL |
| **DPO** (if pairs exist) | repair-ladder runs (spec 09): failed patch + succeeded patch on same error = natural pair | `{prompt, chosen, rejected}` JSONL |

- **P5 — Exclusion rules (non-negotiable, checked at build):**
  1. `context.taint.has_untrusted = true` → excluded (G-01: injection must not launder into weights). Overridable per-row only by explicit human review, logged.
  2. SecretFilter re-scan at build time (defense in depth over RS13's scan-at-write; patterns improve over time and must re-apply retroactively). Any hit → row excluded + incident.
  3. Reward below configured floor → excluded (don't teach mediocrity).
  4. Trajectories older than the current embedding of the *task* (superseded facts, spec 02 M-supersede) → excluded from distillation view.
- **P6 — Dedup + balance:** near-duplicate prompts (content-hash + shingle) deduped keeping highest-reward instance; per-`task_class` counts reported in the manifest so a code-heavy corpus doesn't silently produce a code-only model.
- **P7 — Split:** deterministic hash-split (by `trajectory_id`) into train/val; val never trains. The **frozen eval set (spec 14) is a third, untouchable tier** — never in either split (you can't grade yourself on the test you trained on, S11 spirit).
- **P8 — Dataset manifest:** `{snapshot_hash, view, format, row_count, per-class counts, filter versions (secret-pattern version, taint rules), builder git hash, created_seq}`. The manifest is what training configs reference — never "latest".

## 3. Campaign triggers

- **P9 — Proposal, not initiation:** a campaign *proposal* is emitted when data thresholds are met (config): e.g. ≥ N eligible KTO rows (default 5,000) or ≥ M distillation pairs (default 1,000) accumulated since last campaign, AND estimated cost within budget (CON-11). Proposal = a ledger event + UI surfacing (spec 12 U5) with: dataset stats, method, est. cost, expected benefit hypothesis.
- **P10 — Human decision:** operator approves/declines/defers in UI. Declined proposals record why (feeds threshold tuning). No approval → nothing happens, forever.

## 4. Trainer abstraction

- **P11 — Trainer trait:** training harness behind a trait, same posture as council adapters (spec 05):

```rust
pub trait Trainer {
    fn id(&self) -> &str;                      // 'trl' | 'unsloth' | 'axolotl' | 'art'
    fn supports(&self, method: Method, base: &ModelRef) -> bool;
    fn launch(&self, campaign: &CampaignSpec) -> Result<CampaignHandle>;  // provision + start
    fn poll(&self, h: &CampaignHandle) -> CampaignStatus;                 // metrics stream
    fn fetch_artifact(&self, h: &CampaignHandle) -> Result<AdapterArtifact>; // LoRA safetensors + logs
    fn teardown(&self, h: CampaignHandle) -> Result<()>;                  // ALWAYS releases GPU
}
```

- **P12 — Default trainer: TRL-class** (TRL/unsloth/axolotl — final pick at Phase 9 by then-current maturity); **ART** slots in only for GRPO campaign mode (rollout-based, needs task environments not datasets — see open-questions). One campaign = one method = one trainer.
- **P13 — Campaign job:** kind `train`, normal `jobs` row (durable, lease, timeout measured in hours not minutes). Worker = thin cloud orchestrator: upload dataset snapshot + config → monitor → download artifact → teardown. **Teardown runs on every exit path** including failure/timeout (a leaked GPU rental is a cost incident, G-06).
- **P14 — Egress gate:** dataset upload is a Network-class egress (CON-7): destination must be on the egress allowlist, payload already SecretFilter-clean (P5.2), upload logged with dataset `snapshot_hash`. The cloud training account is project-scoped — no other secrets available to it (S6 posture).

## 5. Training config

- **P15 — Versioned config registry** (like reward weights, spec 06 R10):

```toml
[training.campaign]
method            = "kto"            # kto|dpo|sft_distill|rft|grpo
base_model        = "gemma4-e4b"     # registry id (spec 03 I4)
base_checkpoint   = "<hf-rev-or-hash>"
dataset_snapshot  = "<snapshot_hash>"  # P8 manifest, never 'latest'
lora_rank         = 16
lora_alpha        = 32
learning_rate     = 1e-5
epochs            = 2
quantization      = "q4_k_m"         # for the return path, §6
seed              = 42
max_cost_usd      = 25.0             # hard kill at breach (G-06)
```

- **P16 — Full reproducibility:** `(config hash, dataset snapshot hash, base checkpoint hash, trainer version)` recorded in the campaign ledger event. Same tuple → same artifact (modulo GPU nondeterminism); an artifact whose lineage tuple can't be reproduced is not eligible for activation.

## 6. Return path — adapter → deployable GGUF

The gap most pipelines leave undefined. Ours:

```
LoRA safetensors (cloud) → download + hash → merge into base (fp16)
  → convert_hf_to_gguf.py → quantize (per config P15) → llama.cpp smoke-load
  → registry entry: role=FastCandidate, lineage → §7 gate
```

- **P17 — Conversion is a local job** (kind `model_convert`): merge + GGUF conversion + quantization run locally (CPU-fine, one-off cost) or on the rented box before teardown (config choice). Output artifact named `<base>-<method>-<snapshot8>-<config8>.gguf`, immutable.
- **P18 — Smoke gate before candidacy:** llama-server loads the artifact, runs a 10-prompt smoke set (loads? generates? coherent? tokenizer intact?). Failure → artifact quarantined, never registered. Catches conversion/quant corruption cheaply before burning canary time.
- **P19 — Registry entry:** `ModelSpec` (spec 03 I4) with `role: FastCandidate` + lineage (P16 tuple). A FastCandidate is loadable for evals only — the router never routes production traffic to it.

## 7. Activation gate — a new model IS a self-mod (S10, G-13)

- **P20 — Full chain, no shortcuts:** FastCandidate → **frozen eval canary** (spec 14 E4–E7: safety_invariants hard gate + KPI families + seesaw E6b vs the current fast model's scores) → **council review** of the eval report (spec 05 C6) → human approval (UI, S3) → hot-swap (spec 03 I5) → **post-activation watch** (spec 10 L16): KPI regression beyond threshold → auto-rollback to prior artifact + incident.
- **P21 — Reward-integrity re-run (G-02):** the E8 gameable-task suite runs against the candidate. A fine-tune that learned to game proxies (trained on data that slipped the durability filter) fails here — the last line of defense for the training loop's objective.
- **P22 — Safety invariants are untrainable-away (S11):** provenance gating, tool lockout, SecretFilter live in the harness, not the model — a fine-tune cannot remove them. The canary's safety_invariants block still runs against the candidate to catch *behavioral* regressions (e.g., increased injection compliance) — S1 tests must pass with the new model in the loop.
- **P23 — One candidate at a time:** no parallel candidate models (memory + attention budget); a new campaign proposal while a candidate is in gate → queued behind it.

## 8. Config knobs (→ docs/config.md registry)

| Knob | Default | Rule |
|---|---|---|
| `training.propose_kto_rows` | 5000 | P9 |
| `training.propose_distill_pairs` | 1000 | P9 |
| `training.reward_floor` | config | P5.3 |
| `training.max_cost_usd` | 25.0 | P15, G-06 |
| `training.campaign_timeout_h` | 12 | P13 |

## 9. Acceptance Criteria / Test Anchors (all Phase 9, except T1–T2 buildable earlier)

- [ ] T1: dataset builder excludes a tainted trajectory (`has_untrusted=true`) and a planted-secret row; both exclusions logged. (P5, G-01/CON-13)
- [ ] T2: dataset manifest hash-stable — same trajectory store state → identical `snapshot_hash`; any row change → new hash. (P8/P16)
- [ ] T3: campaign proposal fires at threshold, but NOTHING trains without explicit human approval; declined proposal → no side effects. (P9/P10, CON-8)
- [ ] T4: campaign job teardown releases cloud resources on success, failure, AND timeout paths. (P13, G-06)
- [ ] T5: dataset upload to a non-allowlisted endpoint → refused (egress gate). (P14, CON-7)
- [ ] T6: artifact failing the smoke set → quarantined, never registered. (P18)
- [ ] T7: FastCandidate failing one safety_invariants case → hard-rejected regardless of KPI wins + council. (P20/P22, spec 14 E5)
- [ ] T8: candidate that regresses E8 reward-integrity suite → rejected. (P21, G-02)
- [ ] T9: post-activation KPI regression → auto-rollback to prior artifact; router traffic unaffected during swapback. (P20, spec 03 I5)
- [ ] T10: artifact with unreproducible lineage tuple → ineligible for activation. (P16)
