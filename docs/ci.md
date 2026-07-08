# CI Gates

Defined before code exists so the bar is set, not retrofitted. Every gate maps to a spec/standard.

## Pipeline (on every commit / PR)
1. **Format** — `cargo fmt --check` (`rustfmt.toml`).
2. **Lint** — `cargo clippy -- -D warnings` (`clippy.toml`: denies `unwrap_used`, `expect_used`, `panic` in non-test; see standards).
3. **Supply chain** — `cargo deny check` (`deny.toml`: advisories, licenses, duplicate versions — TM-7).
4. **Build** — native AND portable (catch native-only assumptions, ADR-001).
5. **Test — deterministic layer** — `cargo test` (plumbing T-anchors; replay-mode, seeded — spec 14 E1/E2).
6. **CI-BLOCKING invariant gates** (non-negotiable, red = build fails):
   - `safety_invariants` eval suite (spec 11 S1/T9) — the 5 named invariants.
   - `reward_integrity` eval suite (spec 14 E8, G-02) — gameable tasks must not earn positive reward.
   - **Meta-test:** deleting any invariant test fails the build (spec 11 T9).
7. **Schema check** — structs serialize to `schemas/*.json` cleanly; version bump present if a contract changed.
8. **Secret scan** — SecretFilter regex run over the diff (defense vs committing a key, spec 01 §6 / 11 S4).

## NOT in CI (too flaky / need real model)
- Live-mode model benchmarks (`throughput`, real RAG relevance) — scheduled, logged with variance, never gate (spec 14 E3).
- Qualitative evals — tracked as metrics, not pass/fail (REVIEW RV-08).

## Repo hygiene files (create in Phase 1)
| File | Purpose |
|---|---|
| `rust-toolchain.toml` | pin exact Rust version (reproducibility, G-20) |
| `rustfmt.toml` | format rules |
| `clippy.toml` | lint config (deny list per standards.md) |
| `deny.toml` | cargo-deny: licenses, advisories, dupes (TM-7) |
| `.gitignore` | `target/`, `models/*.gguf`, `*.db`, `*.db-wal`, `artifacts/`, `.env`, `kb/.staging/` |
| `.env.example` | key names only, NO values (CON-9) |
| `README.md` | what/why, WSL2 quickstart, pointer to CLAUDE.md + specs |
| `LICENSE` | project license |

## Commit / branch conventions
- **Conventional Commits** (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`). Scope = crate (`feat(router): …`).
- **Split commits** (user rule / memory): implementation and tracker/spec updates in separate commits.
- Branches: `phase-N/<slice>`, agent worktrees: `agent/<job-id>-<slug>` (spec 08 A4).
- Every commit trailer where relevant: `Localai-Job-Id: <id>` for agent-authored commits (spec 16 RS2 — reward capture depends on it).
- Commit message trailer: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>` when AI-authored.
