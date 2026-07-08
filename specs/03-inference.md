# Spec 03 — Local Inference

**Status:** Draft
**Cites:** CON-1 (mem ceiling), CON-2 (CPU-only), CON-6 (32K ctx) (spec `00`); KPI-04 (throughput), KPI-05 (memory); GAPS G-11 (slot bleed), G-03 (embedding swap), G-20 (determinism).
**Downstream:** `02-memory` (tokenizer, embeddings), `06-router` (self-consistency sampling), all model callers.
**ADRs:** ADR-003 (Gemma 4 12B + MTP), ADR-004 (llama-server HTTP vs FFI).

---

## 1. Runtime topology (ADR-004)

Local model runs as a **separate `llama-server` child process** over `127.0.0.1` HTTP; Brain is an HTTP client (`inference` crate). Rationale in ADR-004:

- Model crash ≠ Brain crash (spec `01` R3, OBJ-5).
- Hot-swap models without restarting Brain (E4B ⇄ 12B).
- llama.cpp's server is its best-maintained, most-featured surface (MTP flags, multimodal, embeddings endpoint).
- FFI (`llama-cpp-rs`) is the **fallback only if** HTTP loopback overhead measurably threatens KPI-04 — decided by benchmark in Phase 2, not by assumption.

Launch (ADR-003 defaults):
```
llama-server -m gemma4-12b-Q4_K_M.gguf \
  --model-draft gemma4-12b-mtp-drafter.gguf --spec-type draft-mtp --spec-draft-n-max 2 \
  --ctx-size 32768 --host 127.0.0.1 --port <cfg> \
  --threads <phys-cores> --mlock   # RUSTFLAGS/native build per CON-2
```

## 2. The single-generation queue (GAPS G-11 — correctness over concurrency)

On a CPU box, concurrent generations don't speed up — they thrash cache and contend for the same cores. Worse, sharing server KV-cache slots across concurrent callers (UI chat + a distiller summary + k=3 self-consistency) risks **cross-request context bleed**.

- **I1** — All generation requests funnel through **one `InferenceQueue` in Brain**; exactly **one generation in flight at a time**. Others wait FIFO with priority (interactive UI > router self-check > background distill). This is a deliberate serialization, not a bug.
- **I2** — Each request sends its **full context every call** (stateless w.r.t. server-side session); no reliance on persistent server slot state for correctness. Server KV reuse (prefix cache) is allowed only as a *transparent* speedup that cannot change output.
- **I3** — Embeddings endpoint is exempt from the generation queue (cheap, parallel-safe) but still bounded by its own semaphore.

## 3. Model registry & hot-swap

```rust
pub struct ModelSpec {
    pub id: String,             // 'gemma4-12b', 'gemma4-e4b'
    pub gguf_path: PathBuf,
    pub draft_path: Option<PathBuf>,
    pub role: ModelRole,        // Primary | Fast | Embedding
    pub ctx: u32, pub est_rss_gb: f32,
}
```

- **I4** — Registry from config; only models whose `est_rss_gb` fits the current MemoryGuard budget (spec `01` R11) may be loaded. Loading Primary + Fast simultaneously is refused if it breaches CON-1 — swap, don't stack.
- **I5** — Hot-swap = spawn new llama-server on alt port → health-check → drain queue to new server → SIGTERM old. Zero dropped requests (queued during drain). Logged as a `model_swap` event.

## 4. Token budget & context assembly

- **I6** — Token counts come from the model's **own tokenizer** via `/tokenize` (spec `02` M4), cached by string hash. No heuristic estimation in any budget-affecting path.
- **I7** — Hard cap 32K (CON-6) enforced *before* send; `ContextManager` (spec `02`) guarantees the assembled prompt fits with ≥4K generation reserve. A request that can't fit after compression is rejected with a clear error, never silently truncated.
- **I8** — Sampling params per call: interactive = temp 0.7; **router self-consistency = temp>0, k=3** (spec `06`); **tests/deterministic paths = temp 0 + fixed seed** (GAPS G-20). Seed logged.

## 5. Multimodal (ADR-003: encoder-free, native image/audio)

- **I9** — Image + audio inputs go through `llama-mtmd-cli`/server multimodal endpoint as native model input (no separate encoder RAM). **Open question for spec `13`:** confirm audio-input depth in llama.cpp's server; if native audio is solid, whisper.cpp is dropped; if not, whisper.cpp stays as the audio front-end. Decision recorded before Phase 5.
- **I10** — Multimodal inputs count against the 32K budget (image/audio tokens are real tokens here); MemoryGuard watches the KV growth from large media inputs.

## 6. Embeddings (GAPS G-03)

- **I11** — Embedding model (fastembed, 384-d, spec `02` M9) is versioned in a `meta` table. Vectors carry `embedding_model_version`.
- **I12** — Retrieval **refuses to compare vectors across versions**. A model change triggers a transactional re-embed job (spec `09`); until it reaches 100%, retrieval falls back to FTS-only with a loud degraded-mode banner. No mixed-space cosine, ever.

## 7. Health, degradation, self-heal hooks

- **I13** — `/health` polled; N consecutive failures → classify (transient → restart llama-server via supervisor; persistent → degraded mode: council-only answers + user alert). Feeds spec `09` ladder.
- **I14** — Generation timeout per request (config, scaled by requested tokens); on timeout → cancel, log, return partial + error, never hang the queue (protects I1 from a stuck generation blocking all others).
- **I15** — Throughput metric (tok/s) sampled per generation → `BrainStatus` + weekly KPI-04 bench log.

## 8. Acceptance Criteria / Test Anchors

- [ ] T1: 3 concurrent callers (UI + distiller + k=3 self-check) → exactly one generation runs at a time; outputs are not cross-contaminated. (I1/I2, G-11)
- [ ] T2: Full-context stateless calls produce identical output regardless of prior request history on the server. (I2)
- [ ] T3: Loading Primary + Fast that together breach CON-1 is refused; swap path stays within budget. (I4)
- [ ] T4: Hot-swap drains queue with zero dropped/failed requests; `model_swap` event logged. (I5)
- [ ] T5: temp=0 + fixed seed → byte-identical generation across runs (deterministic test path). (I8, G-20)
- [ ] T6: Prompt exceeding 32K after compression → rejected with actionable error, never truncated. (I7)
- [ ] T7: Embedding version mismatch → retrieval refuses cross-version compare, falls back to FTS + banner. (I12, G-03)
- [ ] T8: Stuck generation hits timeout → cancelled, queue proceeds, others unblocked. (I14)
- [ ] T9 (bench, not unit): sustained tok/s with MTP on ≥ MTP off, both logged; if HTTP overhead > threshold, ADR-004 FFI fallback triggered. (KPI-04)
