# ADR-003 — Primary Model Selection

**Status:** Accepted
**Date:** 2026-07-06
**Context gate:** spec `00` §5 required verifying the model landscape against real sources before Phase 2. This ADR is that verification.

---

## Correction of prior assumption

`PLAN.md` §1.2 (written from the reviewer's Jan 2026 knowledge cutoff) claimed **"Gemma 4 12B does not exist"** and moved its features to a research-watch list. **This was wrong.** Web verification on 2026-07-06 confirms Gemma 4 shipped and the draft's claims were substantially accurate. The reviewer's cutoff predated the release. This is exactly the failure mode spec `00` §5 was written to catch — decision now rests on sources, not memory.

## Verified facts (2026-07-06)

| Claim | Verdict | Source |
|---|---|---|
| Gemma 4 family exists, released ~April 2026 | **TRUE** | Google AI docs; llama.cpp launch-day support 2026-04-02 |
| Gemma 4 12B is an encoder-free multimodal model | **TRUE** — "replaced vision and audio encoders with direct linear projections of the input" | ai.google.dev/gemma/docs/core |
| Handles text + image + video + audio natively (12B) | **TRUE** — audio native on E2B, E4B, 12B | ai.google.dev/gemma/docs/core |
| Native MTP draft heads for speculative decoding | **TRUE** — "dedicated draft model for speculative decoding… no quality loss"; drafters co-trained, **share activations with target** | Google blog; ai.google.dev |
| llama.cpp supports it (`--spec-type draft-mtp`) | **TRUE** — `--model-draft`, `--spec-type draft-mtp`, `--spec-draft-n-max` | llama.cpp discussion; DEV community |
| MTP speedup ~1.5–3× | **TRUE** (draft said ">50%"; real range 1.5–3×, hardware-dependent) | Google blog; buildfastwithai |
| Context window | 128K (small) / **256K (medium — incl. 12B)** | ai.google.dev |
| Q4 memory | ~8 GB weights; runs on 16 GB RAM; **+~2 GB for MTP drafter** | Unsloth; buildfastwithai |

Draft claims still **unverified** (not blockers, keep on research-watch): "Mesa Layer RLS O(1) attention", "TurboQuant polar-quantized embeddings", the exact "130–500 MB saved vs separate draft" figure (MTP actually costs +~2 GB headroom, but the co-trained activation-sharing drafter is far cheaper than a standalone draft model — spirit correct, exact number unconfirmed).

## Decision

**Primary model: Gemma 4 12B, Q4_K_M GGUF, via llama-server, WITH the co-trained MTP drafter enabled** (`--spec-type draft-mtp`, start `--spec-draft-n-max 2`, tune 1–6 per host).

- Encoder-free multimodal → text/image/audio in one model, no separate vision/audio encoder RAM (retires the need for a separate whisper.cpp path assumed in spec `00` §5 — **revisit spec 00/13**: audio may be native).
- MTP directly serves KPI-04 (throughput) at ~+2 GB RAM — well inside the 22 GB ceiling (CON-1).
- 256K context far exceeds our 32K deployment cap (CON-6); cap stays a *memory* choice, not a model limit.

**Background/fast path:** Gemma 4 **E4B** (effective-4B, mobile-class) as the small model for cheap self-consistency sampling and background logic, replacing the earlier "1B–4B or BitNet" placeholder. One model family = one tokenizer, simpler ops.

## Memory budget impact (revises spec 01 R11)

| Component | Old | New (Gemma 4 12B + MTP) |
|---|---|---|
| Model host | 13 GB | ~8 GB weights + ~3 GB KV@32K + ~2 GB MTP = **~13 GB** | 
| Net | — | **unchanged ceiling; MTP fits in existing model budget** |

## Consequences

- **Revisit spec 00 §5 and spec 13:** audio ingestion may be native to the model — whisper.cpp possibly droppable. Confirm llama.cpp `llama-mtmd-cli` audio support depth before committing.
- **Update PLAN.md §1.2:** strike the "Gemma 4 doesn't exist" row; MTP and encoder-free multimodal move from research-watch to accepted.
- **Speculative decoding is free-ish here** — spec `03` should default MTP on, not treat it as an optional optimization.
- **Re-verify at each phase boundary** (the lesson): model landscape moves faster than any training cutoff. ADR-003 is timestamped; if Gemma 5 or a better CPU model ships before Phase 2 ends, re-open this ADR.

## Sources
- [Gemma 4 model overview — Google AI](https://ai.google.dev/gemma/docs/core)
- [Introducing Gemma 4 12B: encoder-free multimodal — Google blog](https://blog.google/innovation-and-ai/technology/developers-tools/introducing-gemma-4-12b/)
- [Accelerating Gemma 4: multi-token prediction drafters — Google blog](https://blog.google/innovation-and-ai/technology/developers-tools/multi-token-prediction-gemma-4/)
- [2x speed on Gemma 4 with MTP in llama.cpp — DEV](https://dev.to/everylocalai/how-to-get-2x-speed-on-gemma-4-with-multi-token-prediction-in-llamacpp-1b8e)
- [Gemma 4 12B specs & local run — buildfastwithai](https://www.buildfastwithai.com/blogs/gemma-4-12b-guide)
- [Gemma 4 how to run locally — Unsloth](https://unsloth.ai/docs/models/gemma-4)
