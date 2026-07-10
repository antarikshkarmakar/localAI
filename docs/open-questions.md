# Open Questions & Parked Decisions

Registry of decisions deferred to Phase N, research items, and architectural trade-offs still under investigation.

## Research-watch (potential adoption from OSS)

| Item | Source | Rationale | Decide-by |
|---|---|---|---|
| **graphmind as a library** | graphmind (Rust, tree-sitter, SQLite, MIT) | Evaluate whether to import as library vs port pattern to localAI retrieval (spec 02/13). Needs benchmarking. | Phase 3 |
| **HippoRAG** | awesome-foundation-agents / NeurIPS 2024 | Dual-index (temporal vs semantic). May inform our 4-tier split (spec 02). | Phase 5 |
| **DSPy** | awesome-foundation-agents / ICLR 2024 | Demonstration-optimized routing. Alternative to hand-set bandit priors (spec 06). | Phase 7+ |
| **Constitutional AI** | awesome-foundation-agents / 2022 | Self-critique loop + synthetic preference pairs. Replaces manual negative-reward engineering. | Phase 10+ |
| **CubeSandbox eBPF** | CubeSandbox (Rust, RustVMM/KVM, Apache 2.0) | eBPF egress gate + vault patterns. Worth Phase 4+ but cgroup/ulimit suffice for now. | Phase 4+ |
| **Gnap** | awesome-agent-orchestrators | Git repo as task-board. Contrast vs SQLite queue (spec 04). Stay with SQLite for crash recovery. | Phase 2 |
| **LMCache / CacheBlend** | LMCache (Apache 2.0, vLLM-centric) | Library incompatible (GPU/vLLM; we're CPU llama.cpp). Pattern adopted instead: static-prefix KV persistence via llama.cpp natives (spec 03 I2b). CacheBlend (non-prefix KV reuse for RAG chunks) = revisit only if stack ever moves to vLLM/GPU. | Phase 10+ |
| **Harness-evolution automation depth** | HF harness-optimization (Niklaus) | Adopted as spec 10 L10d (eval-driven fitness). Open: how autonomous the mutation loop gets — manual candidates (Phase 6) vs automated rewrite loop (Niklaus-style, needs strong Goodhart guards + eval budget). | Phase 7 |
| **slime (THUDM)** | slime (Megatron+SGLang RL infra) | Infra incompatible (GPU cluster; overkill for 4B QLoRA). Pattern adopted: Data-Buffer → trajectory capture from Day 1 (spec 16 RS12–15, schemas/trajectory.schema.json). For Loop 4 cloud trainer: TRL/unsloth/axolotl class over slime. | Phase 9 |
| **HarnessX / AEGIS** (arXiv 2606.14249) | THU, 2026 | 5 patterns adopted: change manifest (spec 10 L10e), exploration ledger (L10f), seesaw per-item gate (L11b, spec 14 E6b), variant isolation fork-don't-reject (L11c, spec 02 M12), failure digest (spec 09 H1b) + learned ladder entry (H2b), KPI-10 velocity. Open: full typed-processor decomposition of the harness (their Composition Layer) — heavier refactor, revisit when prompt evolution goes live. Failure-derived curriculum (quarantined failures → replayable ART environments) parked with Loop 4. | Phase 6 |
| **dflash block-diffusion drafting** | z-lab/dflash | Diffusion draft model: 15–16 spec tokens/block (vs our MTP `--spec-draft-n-max 2`) — potential KPI-04/RV-03 multiplier. Gemma-4 draft builds exist; MLX backend proves CPU-viable in principle. **Blocker: no llama.cpp backend.** Check llama.cpp diffusion-draft support at every ADR-003 phase-boundary re-verification; if it lands, benchmark vs MTP. | each phase boundary |
| **DiffusionGemma** | Google blog, 2026 | 26B MoE (3.8B active) diffusion text gen, 4× on GPU, quality < Gemma 4, llama.cpp "soon". Skeptical: Q4 ≈ 14–15 GB breaks CON-1 co-residency with 12B; diffusion's parallel refinement passes favor GPU batch compute — 4× may invert on CPU. Only path: llama.cpp lands + CPU bench wins + swap-not-stack usage (background distill bursts). | each phase boundary |
| **Model-specific prompt variants** | Self-Harness (arXiv 2606.09498) finding | Each model needs distinct scaffolding (their 3 models needed disjoint fixes). We run 12B + E4B + council + 3 CLI agents. If per-model divergence shows up in practice, extend variant fork dimension (spec 10 L11c) from `task_class` to `(task_class, model_id)` — same mechanism, one more column. | Phase 6 |
| **Graphiti (getzep)** | Python + Neo4j/FalkorDB — library incompatible (ADR-002 SQLite) | 3 patterns adopted into spec 02 §4.3: bi-temporal fact schema (kg_facts, invalidate-don't-delete), fact-granular audit/supersede (M11d), prescribed+gated-learned ontology (M11b). Open: whether SQLite recursive-CTE traversal suffices at scale vs dedicated graph store — benchmark when KB > ~100k facts. | Phase 5 |
| **train-llm-from-scratch** | FareedKhan-dev | Reference-only: readable from-scratch DPO/PPO/GRPO implementations. Consult when Loop 4 method decisions land (understand method before renting GPU). No production use — toy scale. | Phase 9 (reference) |

## Loop 4 fine-tuning method (Phase 9+, cloud-trained E4B only — spec 10 §1)

Context: no local weight training ever (32 GB CPU). Loop 4 = export trajectories → cloud-tune the **E4B fast model** → run locally via llama.cpp. 12B is never tuned. Every candidate below goes through the full self-mod gate (spec 11 S10: council review → canary vs frozen evals → auto-rollback).

| Candidate | Fit | Notes | Decide-by |
|---|---|---|---|
| **KTO** (Kahneman-Tversky Opt.) | ★ best data-shape match | Works on **unpaired** binary feedback — exactly what spec 16 produces (RS0 corrections, RS2 reverts, RS3 re-edits are thumbs-up/down, NOT paired chosen/rejected for same prompt). DPO needs pairs we mostly won't have at single-user volume. | Phase 9 |
| **DPO / SimPO / ORPO** | good IF pairs exist | Repair-ladder runs (spec 09 §3) DO create natural pairs: failed patch vs succeeded patch on same error. Use for that slice; SimPO = reference-free (cheaper), ORPO = SFT+align in one stage. | Phase 9 |
| **Rejection-sampling SFT (STaR/ReST-style)** | ★ free verified data | Generate k samples → keep only ones passing the *objective* verifier (compiled AND tests pass, spec 06 R8) → SFT on winners. Our reward infra already labels these; training data accumulates as a side effect of normal operation. Anti-gaming inherits from R8 (verifier is not the model). | Phase 9 |
| **Council-as-teacher distillation** | ★ attacks OBJ-2 directly | Every council escalation = (query, council-verified answer) pair — a distillation dataset we're already paying for. Periodically QLoRA E4B on it → local model absorbs what it used to escalate for → escalation rate (KPI-01) drops → cost drops. Flywheel: cloud teaches local. Pairs stored via spec 16 capture; SecretFilter (CON-13) scrubs before export. | Phase 9 |
| **Plain SFT-QLoRA** | baseline | On high-reward trajectories. Simplest; run first as the control arm vs KTO/RFT. | Phase 9 |
| **GRPO via ART (OpenPipe)** | ★ campaign mode | Client-server split matches our topology (local Brain orchestrates, ephemeral cloud GPU trains LoRA via vLLM+Unsloth). Runs as periodic *training campaigns* against task environments with verifiable rewards (code+tests, spec 14 evals) — needs NO user-traffic data (starvation workaround #2), complementary to KTO-on-logged-trajectories. RULER (relative group scoring) removes hand-labeling. **Blockers:** Gemma unsupported by Unsloth path (Loop 4 target = Gemma 4 E4B per ADR-003 — re-check support or shift target model, ADR-003 addendum); on-policy rollouts can't consume our offline RS12 logs. | Phase 9 |

**Recommended composite (pre-decision, revisit Phase 9):** council-distillation + rejection-sampling SFT as the data recipe, KTO as the objective, QLoRA 4-bit as the method, E4B as the only target. Cheap cloud run (~$5–20/epoch at 4B scale), canary-gated like any self-mod.

## Serving-side "tuning" (no training — earlier phases, cheap wins)

| Technique | Why good for us | Decide-by |
|---|---|---|
| **Dynamic few-shot from episodic memory** (many-shot ICL) | "Fine-tuning without fine-tuning": retrieve past *successful* solutions (reward-positive episodes, spec 02) as in-prompt examples for similar new tasks. Zero training, works day 1 after memory fills, compounds with KB. Candidate for spec 02 M11 retrieval + spec 10 addition. | Phase 4 |
| **LoRA adapter hot-swap at inference** | llama.cpp serves base + per-task-class LoRA adapters (`--lora`), swappable without reload. Router (spec 06) picks adapter like it picks route. Turns one E4B into N specialists at ~100 MB/adapter. Only relevant once Loop 4 produces adapters. | Phase 9 |
| **Best-of-N + verifier rerank** | Spend tokens not weights: k samples → objective verifier (tests/council/auditor) picks. Already partially in LOCAL_SELFCHECK (spec 06); extend with verifier-rerank instead of majority-vote where a checkable oracle exists. | Phase 5 |

## Architecture trade-offs (still open)

| Decision | Options | Status | Decide-by |
|---|---|---|---|
| **Audio input** | Native Gemma 4 audio vs whisper.cpp | Pending Phase-5 llama.cpp test (ADR-003). | Phase 5 |
| **Model residency** | E4B hot + 12B on-demand vs 12B primary | Chosen in REVIEW RV-04. Confirm Phase 1.5. | Phase 2 |
| **Routing algo** | Thompson bandit vs rule-tree | Chosen in spec 06. Low data may change priority. | Phase 7 |
| **Vector storage** | sqlite-vec vs Qdrant | Chosen in ADR-002. Revisit if perf bottleneck. | Phase 4 |
| **Model invocation** | llama-server HTTP vs Rust FFI | Chosen in ADR-004. Benchmark loopback vs KPI-04. | Phase 2 |

## Deferred features (not Phase 1–3)

| Feature | Why later |
|---|---|
| Local fine-tuning (LoRA) | Phase 9+ (cloud QLoRA on trajectories). CPU too slow. See "Loop 4 fine-tuning method" above. |
| Multi-modal output | Gemma 4 input-only. Needs separate model. |
| Distributed agents | Single-workstation focus (OBJ-1). |
| Cloud sync | Manual export only; write-once local design. |
