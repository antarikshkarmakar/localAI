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
| Local fine-tuning (LoRA) | Phase 9+ (cloud QLoRA on trajectories). CPU too slow. |
| Multi-modal output | Gemma 4 input-only. Needs separate model. |
| Distributed agents | Single-workstation focus (OBJ-1). |
| Cloud sync | Manual export only; write-once local design. |
