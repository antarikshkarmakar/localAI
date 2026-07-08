# ADR-001 — No Docker for the LLM/Brain runtime

**Status:** Accepted
**Date:** 2026-07-06
**Cites:** CON-1 (22 GB ceiling), CON-2 (CPU-only), CON-3 (WSL2 bare metal).

## Context
32 GB Windows host, CPU-only inference via WSL2. Docker Desktop on Windows runs its own hidden WSL2 utility VM.

## Decision
Run the Rust Brain + llama-server **directly on bare metal inside WSL2 (Ubuntu 24.04)**. Do **not** containerize the LLM or Brain. Auxiliary tools only (heavy headless-browser scrapers, static dashboards) MAY run in Docker, interacting over localhost ports.

## Rationale
- **Memory:** Docker Desktop's subsystem passively consumes ~2–4 GB before any model loads. On a 22 GB budget (CON-1) that's the difference between resident and swapping. Windows paging → LLM generation drops from stream to crawl.
- **CPU SIMD:** native build with `RUSTFLAGS="-C target-cpu=native"` + llama.cpp compiled for the exact chip (AVX-512/AVX2/AMX). Generic prebuilt container images target broad compatibility → leave throughput (KPI-04) on the table.
- **Filesystem:** SQLite + OKF on the Linux fs at native speed; container volume translation adds latency and lock-semantics risk.

## Consequences
- Setup is WSL2-native (documented in runbook); not portable to a container host without rework — acceptable, this is a single-workstation system.
- Auxiliary containers are allowed but must be optional and localhost-scoped (PLAN hybrid layout, spec 13 D7).
