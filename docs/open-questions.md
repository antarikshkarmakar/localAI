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
| **DiffusionGemma** | Google blog + official deepmind.google page | 26B MoE (3.8B active) diffusion text gen; official numbers: **24 GB VRAM quantized**, 1,000+ tok/s on H100, 256 tokens/forward-pass, bi-directional attention, NVFP4/Blackwell-native. No CPU support, no llama.cpp/GGUF, no smaller variants on official page (blog's "llama.cpp soon" absent there). All batch-compute patterns — GPU-shaped, CPU-hostile; 24 GB breaks CON-1 outright. Only path: llama.cpp lands + a small variant ships + CPU bench wins + swap-not-stack usage. | each phase boundary |
| **SkillOpt** (arXiv 2605.23904) | executive skill-optimization | Adopted as spec 10 L10g: improvement-effort scheduler — nightly eval budget allocated by (bundle recurrence × KPI impact × inverse eval cost); generators propose, executive disposes. Open: skill *transferability* tracking — derive from per-task_class variant stats (L11c fork data) instead of new machinery. | Phase 7 |
| **shepherd** (shepherd-agents) | Python execution substrate, MIT | 2 patterns adopted (spec 08 A5b declared path grants, A6b Landlock kernel fs sandbox); rest convergent (retained proposals=A15, CoW worktrees=A4, replay traces=E1/RS12). Landlock needs WSL2 kernel ≥5.13 — verify at Phase 4; interim = env-scrub + worktree isolation. Not a dependency (Python; we're Rust). | Phase 4 |
| **antarikshSkills v2 rollout** | own skills repo | 24 /ak-* skills seed the Brain process library; upgrade frontmatter in place (verify/privileged/max_provenance/task_class/budget/grants) skill-by-skill as each is automated — NO fork, dual-use is the Phase-1.5 design. Which skill automates first + field schema finalization. | Phase 1.5→2 |
| **PDR / Parallel-Distill-Refine** (arXiv 2510.01123) | adopted spec 06 R1b | LOCAL_SELFCHECK = draft k → distill (E4B) → refine (12B), bounded workspace — beats majority vote at matched compute, keeps refine context small. Open: their training-alignment result (RL-tune the model FOR PDR orchestration) is a Loop 4 method candidate — fold into Phase 9 method bake-off if PDR route proves out live. | Phase 5 impl / Phase 9 training |
| **Idea-distribution gap** (arXiv 2607.01233) | grounding for L10f | LLM proposers cluster on bridge/synthesis moves — K candidates inherit the bias; exploration-ledger dimension-forcing is the structural counter (noted in L10f). Also the empirical case for the preference layer (§5.1): taste is what the model can't self-generate. No further action. | — |
| **Kernel norm-fusion** (PyTorch blog, Meta) | REJECTED — wrong layer | Fusing RMSNorm/LayerNorm into GEMM/attention kernels: Blackwell-specific (DSMEM/CTA/TMA), and kernels are llama.cpp/ggml's layer, not ours (ADR-004 exists to NOT own this). Our lever for this gain class: track llama.cpp releases (fusion lands upstream), native build (CON-2), weekly KPI-04 bench catches deltas. Building kernels in Rust = the RV-01 scope trap. | — |
| **Model-specific prompt variants** | Self-Harness (arXiv 2606.09498) finding | Each model needs distinct scaffolding (their 3 models needed disjoint fixes). We run 12B + E4B + council + 3 CLI agents. If per-model divergence shows up in practice, extend variant fork dimension (spec 10 L11c) from `task_class` to `(task_class, model_id)` — same mechanism, one more column. | Phase 6 |
| **Graphiti (getzep)** | Python + Neo4j/FalkorDB — library incompatible (ADR-002 SQLite) | 3 patterns adopted into spec 02 §4.3: bi-temporal fact schema (kg_facts, invalidate-don't-delete), fact-granular audit/supersede (M11d), prescribed+gated-learned ontology (M11b). Open: whether SQLite recursive-CTE traversal suffices at scale vs dedicated graph store — benchmark when KB > ~100k facts. | Phase 5 |
| **Proactive Memory Agent** (arXiv 2607.08716) | ★★ adopt at memory build — priority raised | "Behavioral state decay": constraints/lessons buried beyond context in long-horizon runs. Separate memory agent watches trajectory + **actively injects reminders at the right moment**; selective intervention beat passive retrieval AND continuous injection (+8.3pp Terminal-Bench). Our M11 is passive-only — this is the missing active half. Target: spec 02 working-memory rule at Phase 4; E4B as watcher (executor≠reviewer); highest value in repair-ladder + long agent runs (spec 08/09). **Second independent signal:** Mercury's "dormant memory resurfaces when relevant" (mercuryagent.sh Second Brain) names the same gap — two sources, one hole. Caveat when designing: dynamic reminder injection must land AFTER the static prefix (spec 03 I2b) or it destroys KV prefix-cache reuse — token-minimal ≠ latency-minimal on CPU. | Phase 4 |
| **fabric (danielmiessler)** | MIT prompt-pattern library | Adopted as spec 10 L18b: curated seed corpus for cold-start prompt library (distill/summarize/extract patterns for spec 13 pipeline). Human-reviewed subset, source-attributed, never bulk import. | Phase 6 seed |
| **colibri (JustVugg)** | SSD-streaming MoE inference | Runs 744B-class at ~1 tok/s on laptop — exploration toy only; violates KPI-04 (≥6 tok/s), useless for distillation (~11 days/1M tokens). Confirms ADR-003 right-sizing + spec 17 P2 (cloud GPU training). No adoption. | — |
| **Qwen-3.5-Opus-GLM-27B merge** | HF community merge (i1 GGUF) | Candidate-list only for ADR-003 phase-boundary re-verification. Against: community merge unverified quality; 27B Q4 ≈ 16 GB breaks ≤13 GB model envelope; ~3 tok/s CPU fails KPI-04. | each phase boundary |
| **agent-scripts (steipete)** | conventions repo | 2 cheap adoptions when relevant: (a) CI schema-validation of prompt-library/skill files (validate-skills pattern) — add to docs/ci.md step 7 scope at Phase 6; (b) briefs reference canonical constraint docs instead of copying (spec 08 A8 hygiene, no drift). | Phase 6 |
| **Judgment library** (Pachaar thread) | X/@akshay_pachaar | Adopted as spec 02 §5.1 `preferences` table: taste as a distinct layer (facts=what's true, recipes=how, preferences=what's *quality*), explicit-save-only (M12b, ties RS0), consulted-first (M12c), self-audited (M12d). Metric: preference_set_size (spec 14). | Phase 4 |
| **DeepHat-V1-7B** | HF, Qwen2.5-Coder-7B security fine-tune | Only CPU-runnable model in this batch (GGUF, ~5 GB Q4). Candidate as a LOCAL security-aware fast model for pre-council triage (spec 05/09) — but security currently routes to council; adopt only if local security screening proves needed + council cost too high. Apache-2.0 + use restrictions. | Phase 8 |
| **NVFP4 models** (Qwen3.6-27B, gemma-4-31B) | unsloth / RedHatAI | REJECTED: NVFP4 = W4A4 on Blackwell tensor cores, GPU-exclusive, explicitly NOT llama.cpp/CPU (unsloth docs). gemma-4-31B also breaks ≤13 GB envelope. Recorded so not re-litigated: our quant path is GGUF/imatrix; GPU-format quant race doesn't cross to CPU. | — |
| **GLM-5.2 Thireus splits** | HF, GGUF tool suite | REJECTED: 744B-class (colibri territory), memory-impossible on 32 GB regardless of quant. | — |
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
| **ReOPD** (arXiv 2607.04763) | ★ trajectory-native distill | Replayed-Prefix On-Policy Distillation: student learns from **pre-collected teacher trajectories** (our RS12 capture) with dense per-step supervision, ZERO environment re-runs, ≥4× faster than online OPD. Directly consumes what spec 16 already stores — the cheapest path from "we logged trajectories" to "we trained a smaller local model." Prime DGX-local Loop-4 candidate; step-decay prefix schedule avoids the reliability/occupancy shift. | Phase 9 (DGX-local) |
| **autoresearch / AgentHub** (Karpathy) | ★ the DGX Loop-4 *shape* | The concrete pattern for training on the DGX: a single-GPU harness (fixed prepare/eval, mutable train.py, program.md policy) + a git-ratchet loop (inspect→propose→apply→eval→keep-or-revert). Verifiable output, reversible action, short horizon, bounded env = agent-compatible. Our repair ladder + L10 evolution already ARE a ratchet loop; autoresearch is that loop pointed at model training. Use as spec 17 local-training reference. | Phase 9 (DGX) |
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

## DGX Spark reframing (hardware inbound — re-open at acquisition)

The operator is acquiring an **NVIDIA DGX Spark (GB10 Grace-Blackwell, ~128 GB unified memory)**. This does NOT change the architecture — provenance gate, council, reward loop, memory tiers, crash-safety are hardware-independent and run now on the frozen local model. It relaxes exactly two things, re-open both at acquisition:

| Constraint | Now | On DGX | Action |
|---|---|---|---|
| **CON-1** memory ceiling | 22 GB (RV-04: E4B-hot / 12B-on-demand) | ~128 GB | Re-cost residency — hold 12B + drafter + embeddings + headroom resident; MTP always-on. Re-open ADR-003/RV-04. |
| **Loop 4 location** | cloud QLoRA (spec 17 P2) | on-box QLoRA/KTO/ReOPD | spec 17's `Trainer` trait (P11) already abstracts this — swap the cloud campaign for a local one. autoresearch shape (above) + ReOPD method (above). |

Clarification the design already encodes (worth restating): this is **not** "train, then use." Loops 1–3 (knowledge/decision/procedural, spec 10) learn continuously on a frozen model with zero GPU — the KB compounds, the bandit re-weights, prompts evolve. The DGX only unlocks the *optional* Loop-4 weight training. Build the self-* loops now; let the DGX absorb Loop 4.

## Rejected / watch — model-architecture & kernel papers (wrong layer for us)

| Paper | Verdict |
|---|---|
| **MHAR** (2607.27230) Multi-Head Attention Residuals | REJECT: attention-architecture change, train-from-scratch. We run Gemma, don't pretrain base models. Only relevant if the plan ever includes pretraining — it doesn't. |
| **Kernel Forge** (2607.24762) CUDA kernel opt via MCTS | REJECT (wrong layer, like PyTorch norm-fusion): llama.cpp/ggml owns kernels (ADR-004). Notable only because it was benchmarked ON DGX Spark GB10 — confirms the operator's hardware runs this workload class. |
| **Speculate While You Reason** (2607.25816) self-speculating agent | WATCH: "the agent is its own best next-tool-call speculator," shared prefix-KV. Latency win on tool-heavy paths; marginal at single-user CPU volume, pairs conceptually with the MTP drafter (ADR-003). Revisit if tool-call latency becomes a measured bottleneck. |

## Paper-corpus triage (C:\GitHub\papers, ~70 PDFs — 2026-08 batch)

Deep-read + adopted (design-changing): **Sample More, Reflect Less** (2607.28576 → spec 06 R1b correction: sample-and-select over self-critique at our scale; spec 09 rung 1). Validated existing design: **MemHarness** (2607.28272 → spec 02 M11g reconstruct-not-replay), **Harness-R1** (2608.02276 → cousin of Living-Harness H1c; L11b seesaw already gates "edits can hurt"), **Verification Horizon** (2606.26300 → spec 14 E15b no-perfect-verifier).

Watch buckets (skimmed by title/abstract — revisit at the phase noted):

| Cluster | Papers | For us | When |
|---|---|---|---|
| **Memory foundation models** | Metis (2607.26760), Memory Decoder at Scale (2607.27919), Neural Procedural Memory (2606.29824) | Parametric/pretrained long-term memory — trainable memory modules. Only relevant if we train a memory model on the DGX; our KB+kg_* is the non-parametric equivalent now. | Phase 9 (DGX) |
| **Agent memory systems** | Filesystem-Based Memory (2607.26637), Are We Ready for Agent-Native Memory (2606.24775), Always-On Agents survey (2606.30306), PRO-LONG (2607.20064), From Memory to Skills co-evolution (2607.16621) | Cross-check our 4-tier + OKF + kg_* against these designs; From-Memory-to-Skills' evidence-grounded governance aligns with our CoE (L1b). No change now; audit at memory-build. | Phase 4–5 |
| **Harness self-improvement** | Harness Handbook (2607.13285), REHEARSE confidence-cliff (2607.27687), Frontis-MA1 recursive SI (2607.28568), Bilevel Autoresearch (2603.23420), SkillSmith (2607.27497), Generative Skill Composition (2606.32025) | REHEARSE (self-improvement stability/confidence-cliff) + Frontis-MA1 (recursive SI in MLE) are the DGX-Loop-4 self-improvement references; skill-composition papers inform prompt_library structure. | Phase 6 / 9 |
| **Loop-4 distillation/RL** | DAPD (2608.01735), Weak-to-Strong OPD (2607.26246), Reusing Rollouts / Prefix-Normalized PO (2608.01418), ReOPD (2607.04763, already) | Method bake-off for training the E4B on DGX from RS12 trajectories. Weak-to-Strong + ReOPD + prefix-normalized are the strongest trajectory-native candidates. | Phase 9 (DGX) |
| **Context/failure localization** | ACM (2607.23809), Progressive Disclosure (2607.17598), Context Fails First (2607.14275), Model-or-Harness taxonomy (2607.28802), Role Drift (2607.21627) | Reinforce M3b masking + Living-Harness regime map + G-17 (context poisoning); Context-Fails-First matches our provenance-first stance. Audit at UI/observability. | Phase 6 |
| **Verification/reward** | RLVR→RLSVR self-verifiable (2607.23802), RL for Code Opt (2607.25970), Verification Horizon (done) | Reward-design references for the bandit (spec 06); RLSVR's self-verifiable-reward transformation is a Loop-4 reward candidate. | Phase 7 / 9 |
| **Agent patterns (validation)** | Andrew Ng loops→graphs, Graph-of-Agents (2604.17148), NVIDIA OO Agents (2607.20709), Science of Scaling Agent Systems (2512.08296) | Convergent with our supervisor/worker + kg_* + Karpathy graph-engineering. Validation, no new primitive. | — |

Rejected — wrong layer (vision / attention-architecture / pretraining-scales / kernel-opt, all below ADR-004's line): Chimera, ReToken (vision); MHAR, LongCat, SparDA (attention arch); SOAP-Muon, Small-LLMs-Pruning, Requential-Coding, Explorative-Modeling (pretraining); Kernel Forge, JAXBench (kernel/TPU); Molt, Native-training-frameworks (GPU RL infra); plus interp/eval-only papers (Verbalizable Representations, Not-All-Reasoning-Visible, Structured-Output-Collapses, Frontier-Models-Struggle-to-Copy). None touch our CPU/orchestration/memory layer.

**docx guides** (4-Layer Memory, Subagent Layer Fix, YC Agent Harness) — operator's own notes; cross-reference against spec 02 (memory tiers) + spec 08 (agents) when those phases build; not auto-adopted (unverified provenance, may contain project-specific context).
