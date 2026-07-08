# Engineering Standards

Normative conventions for all `localai` code. Deviations need a comment justifying them. Enforced by `clippy.toml` + CI (`docs/ci.md`) where mechanizable.

## Language & edition
- Rust, edition 2021+ (2024 when stable-adopted), `rust-toolchain.toml` pins the exact version — reproducible builds (spec 14 G-20).
- Native build for the LLM host: `RUSTFLAGS="-C target-cpu=native"` (ADR-001). CI also runs a portable build to catch native-only assumptions.

## Error handling
- **Library crates** (`core`, `ledger`, `store`, `inference`, …): typed errors via `thiserror`. One error enum per crate; variants map to spec 09's taxonomy (`transient|input|bug|resource`) where they cross the job boundary.
- **Binary crates** (`brain`, `workers`): `anyhow` at the top level only, for context-rich exit.
- **`unwrap()` / `expect()` / `panic!` are FORBIDDEN** outside tests and `main` startup asserts. A recoverable path that panics is a bug (crashes the single-writer Brain, spec 01). Clippy denies `unwrap_used`, `expect_used` in non-test code.
- Every fallible boundary returns `Result`; no sentinel values, no silent `Option` drops on error paths.

## Time (GAPS G-09 — load-bearing)
- **Never compute a duration from wall-clock alone.** Ordering authority = SQLite rowid / a monotonic `seq`. Store wall-clock `ts` for display only.
- Detect large backward/forward clock jumps (WSL sleep/resume) and flag affected windows.
- No `SystemTime::now()` in reward/hold-window logic — use `seq` (spec 16 RS4).

## Logging & tracing
- `tracing` crate. **Every ledger event has a corresponding tracing span**; `trace_id` (spec 12 U3) propagates through spans so the Explain view and logs align.
- Log levels: `error`=incident (spec 11 S14), `warn`=degraded/retry, `info`=state transitions, `debug`=dev. No `println!` outside the UI notification path.
- **Never log secrets** — the SecretFilter (spec 11 S5) also guards the log sink. Structured fields only; no string-concatenated payloads that could smuggle a secret past the filter.

## Concurrency
- One dispatch loop mutates Brain state (spec 01 R7). Shared state via `Arc<RwLock<…>>` held for microseconds (draft's granular-locking principle) — never across an `.await` that does I/O.
- Channels bounded (spec 01 R9); document backpressure behavior at each channel.
- No unbounded `tokio::spawn`; workers go through the supervisor + semaphore (spec 04 O4).

## Traits & testability (spec 01 R6)
- Cross-subsystem calls go through `core` traits (`Inference`, `CouncilMember`, `Tool`, `ToolDispatch`). Concrete impls injected in `brain`. This is what makes the deterministic layer TDD-able (REVIEW RV-08).
- `core` has **zero I/O deps** — enforced by a `cargo tree` assertion test (spec 01 T1).

## Contracts
- Wire/storage formats validate against `schemas/*.json` (versioned). A struct that serializes to a contract has a test asserting it matches the schema. Schema change → version bump → consumer check.

## Naming & layout
- Crates: `snake_case`, one responsibility (spec 01 §2). Modules mirror spec sections where practical.
- Public API minimal; `pub(crate)` by default. Document every `pub` item with the spec rule it implements (`/// Spec 04 O2: …`).

## Dependencies (TM-7, supply chain)
- `cargo-deny` (`deny.toml`) gates licenses + advisories + duplicate versions in CI.
- New dep needs justification in the PR. Model files + agent binaries pinned by checksum (spec 11 TM-7).
- Prefer std + a small vetted set over sprawling trees; every dep is attack surface.

## Testing (REVIEW RV-08 split)
- **Deterministic plumbing** (queue, ledger, provenance gate, budget guard, schema, crash-safety) → **strict TDD**, test-first, the T-anchors in each spec.
- **Qualitative behavior** (distill/RAG/fact-check/answer quality) → **eval-driven** (spec 14), scored + tracked, NOT pass/fail CI gates.
- **Safety-invariant + reward-integrity tests are CI-blocking** (spec 11 T9, spec 14 E11). Red = build fails.
- Determinism: seed all RNG in tests; model/council behind mocks; record/replay for regression (spec 14 E1).
