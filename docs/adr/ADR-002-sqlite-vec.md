# ADR-002 — SQLite + sqlite-vec as the sole datastore

**Status:** Accepted
**Date:** 2026-07-06
**Cites:** OBJ-1 (sovereignty), CON-1 (memory), CON-4 (Linux fs).

## Context
Two storage needs: structured metadata (OKF tags, links, job/ledger tables) and vectors (RAG embeddings). Alternatives: Postgres+pgvector, Qdrant, standalone vector DBs.

## Decision
Single embedded **SQLite** file (WAL mode) with the **sqlite-vec** extension for vectors and FTS5 for keyword search. No standalone DB server. One `.db` on the Linux filesystem (CON-4, never `/mnt/c`).

## Rationale
- **Zero idle RAM:** in-process; a server DB (Postgres/Qdrant in a container) holds 1–4 GB continuously — unaffordable on the 22 GB budget (CON-1). SQLite consumes memory only during a query.
- **One file, one truth-index:** relational + vector + FTS in one transactional file simplifies backup, reconciliation (spec 09 H5), and crash-safety (single WAL).
- **sqlite-vec** (not the deprecated sqlite-vss — draft's "Vector Search Sequential" expansion was wrong): actively maintained, int8/binary quantization support (spec 02 M9).
- OKF Markdown files remain ground truth (spec 02 M1); the DB is a rebuildable index — resilience without a heavyweight DB.

## Consequences
- No concurrent multi-writer: mitigated by single-writer Brain (spec 01 R1) + WAL for concurrent reads.
- WAL growth under load needs maintenance (spec 09 H7, GAPS G-08).
- Vector search is CPU brute-force/ANN within SQLite — fine at personal-KB scale; revisit only if the KB grows beyond what int8 + ANN handles.
