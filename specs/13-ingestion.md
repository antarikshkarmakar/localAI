# Spec 13 — Web Scraping & Document Ingestion

**Status:** Draft
**Cites:** OBJ-3 (compounding), CON-5 (≤3 parallel), CON-7 (egress), CON-10 (untrusted) (spec `00`); GAPS G-15 (scraper reality), G-01 (injection).
**Depends on:** `04-orchestration` (scrape/ingest/distill jobs), `07-harness` (provenance, egress), `03-inference` (multimodal, embeddings), `02-memory` (chunking, OKF).
**Resolves:** ADR-003 open question — native audio vs whisper.cpp.

---

## 1. Pipeline

```
source → scrape (Untrusted) → extract/clean → chunk → embed → OKF draft → [fact-check, spec 10] 
```
Runs as three job kinds (spec 04): `scrape`, `ingest` (clean+chunk+embed), `distill` (→ OKF). All output `Untrusted` provenance (spec 07 O9/H7).

## 2. Scraper etiquette & reality (GAPS G-15)

- **D1 — Allowlist + robots:** scrape targets checked against a scrape-allowlist and the site's `robots.txt`/`noindex` before fetch (CON-7). Non-allowlisted host → denied + logged.
- **D2 — Politeness:** per-domain rate limit + backoff; concurrent scrapes bounded by the global semaphore (≤3, CON-5) AND a per-domain limit (never hammer one host).
- **D3 — Ban/wall detection:** 403/429/Cloudflare/JS-wall/login-wall detected → stop hitting that domain, mark it, back off long. Don't retry into a ban (wastes the loop, risks IP block).
- **D4 — Content dedup (G-15):** content-hash every fetch; unchanged since last crawl → skip distill (spec 10 L2). No re-learning the same page.
- **D5 — Source-quality score:** per-domain score from downstream audit outcomes (spec 10 L2) down-ranks low-signal/unreliable domains over time. The scraper learns where to look.
- **D6 — Injection awareness (G-01):** scraped text is inert data end-to-end (spec 07 H3). A page trying to inject instructions is stored, chunked, maybe summarized — never obeyed, never grants privileged tools. This is enforced upstream (harness); ingestion just tags provenance correctly and never "executes" page content.

## 3. Fetch mechanisms

- **D7 — Tiered fetch:** static HTML → `reqwest` + `scraper` crate (cheap). JS-rendered → headless browser (heavier, gated behind a flag + its own resource cap; optional Docker-isolated per PLAN hybrid). Prefer the cheapest that gets clean content.
- **D8 — Search integration:** `web_search` tool (spec 07, Network class) returns candidate URLs → scrape jobs enqueued for the promising ones (router SEARCH route, spec 06).

## 4. Extraction & structural parsing

- **D9 — Code → tree-sitter AST** (kept from draft FR-04): scraped/ingested source files parsed to ASTs so the agent targets specific blocks, not raw boilerplate. AST nodes are chunkable units.
- **D10 — Documents/PDF → VLM OCR:** complex layouts, charts, tables via a document-centric VLM (olmOCR / docling class) → clean Markdown + LaTeX. Runs through the multimodal model path (spec 03 I9) where possible.
- **D11 — Boilerplate strip:** nav/ads/chrome removed (readability-style) before chunking — don't embed noise.
- **D11b — Chunk enrichment at ingest (agentic-rag pattern):** each chunk gets companion data before embedding: (a) LLM-generated summary (≤50 tokens, concise intent), (b) entity tags extracted/classified (proper names, concepts, types), (c) optional hypothetical questions this chunk could answer. Stored alongside the raw chunk in the `rag_chunks` table. Improves vector retrieval accuracy (more signal in the embedding) + enables retrieval grading (auditor can score on enriched metadata, not just raw text). Cheap when done at ingest scale.

## 5. Audio — resolves ADR-003 open question

- **D12 — Decision (pending Phase-5 confirmation):** Gemma 4 12B is encoder-free with **native audio input** (ADR-003). Primary path = feed 16 kHz audio directly to the model (spec 03 I9), NO separate whisper.cpp — saves ~1 GB RAM + a dependency.
- **D13 — Fallback:** IF Phase-5 testing shows llama.cpp server audio-input support is immature/unstable, whisper.cpp is reinstated as the audio front-end (transcribe → text → model). The decision is recorded in ADR-003 addendum before Phase 5 ships; spec 00 §5 updated to match. Do not build both speculatively — confirm, then pick one.

## 6. Chunking (spec 02 M10)

- **D14** — Target ~350 tokens, 15% overlap, never split inside a code fence or table; chunk 0 = title + frontmatter summary. Token counts via the real tokenizer (spec 03 I6).

## 7. Rate/cost/legal guardrails

- **D15 — Copyright/paywall respect:** paywalled/`noindex`/clearly-licensed-restricted content is not stored wholesale; store citations + short quotes + derived summaries, not full copies. Sovereignty is about *our* data (OBJ-1), not appropriating others'.
- **D16 — Poisoned-page resilience:** a page targeting scrapers (zip bombs, giant payloads, malformed encodings) is size-capped and sandboxed in the scraper worker (cgroup, spec 04 O7); an over-limit fetch aborts as `input`-class failure (spec 09).

## 8. Acceptance Criteria / Test Anchors

- [ ] T1: non-allowlisted host scrape → denied + logged; allowlisted → proceeds. (D1, CON-7)
- [ ] T2: same page fetched twice (unchanged hash) → second fetch skips distill. (D4, G-15)
- [ ] T3: domain returning 429/Cloudflare → scraper backs off, marks domain, stops retrying into the ban. (D3)
- [ ] T4: page with injected "ignore instructions" text → stored + chunked as inert `Untrusted`; never triggers a tool. (D6, G-01)
- [ ] T5: code file → tree-sitter AST chunks align to functions/blocks, not arbitrary byte offsets. (D9)
- [ ] T6: oversized/poisoned payload → size-capped, aborts as input-class failure, worker unharmed. (D16)
- [ ] T7: audio path — native model input produces a transcript-quality result; if it fails the Phase-5 bar, whisper.cpp fallback documented + wired. (D12/D13)
- [ ] T8: paywalled content → citation + summary stored, not full-text copy. (D15)
