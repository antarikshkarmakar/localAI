# ADR-004 — llama-server HTTP vs FFI for local inference

**Status:** Accepted (with a Phase-2 benchmark re-open clause)
**Date:** 2026-07-06
**Cites:** OBJ-5 (resilience), KPI-04 (throughput); spec 03.

## Context
Two ways for the Rust Brain to drive llama.cpp: (a) spawn `llama-server` and talk HTTP over loopback; (b) link llama.cpp via FFI (`llama-cpp-rs`) in-process.

## Decision
**Default: `llama-server` child process over `127.0.0.1` HTTP.** FFI is the fallback, adopted only if a Phase-2 benchmark shows loopback HTTP overhead measurably threatens KPI-04 (≥6 tok/s).

## Rationale
- **Fault isolation (OBJ-5):** model crash/hang ≠ Brain crash. Supervisor restarts llama-server independently (spec 09 L1).
- **Hot-swap:** switch E4B⇄12B by draining to a new server process (spec 03 I5) — impossible cleanly with in-process FFI.
- **Feature surface:** llama.cpp's server exposes MTP flags (`--spec-type draft-mtp`, ADR-003), multimodal (`llama-mtmd-cli`), embeddings, `/tokenize`, `/health` — the best-maintained interface.
- **Loopback overhead is small** relative to CPU generation time for a 12B model; the single-generation queue (spec 03 I1) means we're not paying per-token round-trips, just per-request.

## Consequences
- Full context sent per request (spec 03 I2) — deliberate, for correctness (no server-slot bleed, GAPS G-11).
- One extra process to supervise (already have the supervision tree, spec 04).
- **Re-open trigger:** if Phase-2 `throughput` eval (spec 14) shows HTTP framing/serialization steals a meaningful fraction of tok/s vs an FFI spike, switch the `inference` crate's backend impl behind the same `core::Inference` trait (spec 01 R6 makes this a localized change, not a rewrite).
