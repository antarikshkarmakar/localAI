# Dev Skills (Claude Code) — Specification

Claude Code skills for the dev workflow (PLAN §12). Scaffolded as `.claude/skills/<name>/SKILL.md` in **Phase 1** once there's a CLI + DB to query — premature stubs would be empty. This is their contract.

| Skill | Trigger | Does | Depends on |
|---|---|---|---|
| `/spec <n>` | `/spec 04` | Load spec N, restate its acceptance T-anchors, start the TDD loop (failing test first) | specs/ |
| `/bench` | `/bench` | Run tok/s + RAM benchmark, append to metrics log | `localai bench`, spec 14 throughput |
| `/ledger <query>` | `/ledger route today` | Query the activity ledger, render events + causal trace | `localai ledger`, spec 02 |
| `/audit-facts` | `/audit-facts` | Trigger fact calibration audit (spec 05 mode 4) | council, spec 05 |
| `/handoff` | session end | Generate HANDOFF.md from the session (schema-valid, spec 08) | schemas/handoff |
| `/explain <trace_id>` | `/explain abc123` | Reconstruct why an answer happened — retrieval, route, sources, cost (G-18) | ledger, spec 12 U3 |
| `/oq` | `/oq` | List/close open questions (docs/open-questions.md) | — |

**Convention:** each skill is thin — it shells to a `localai` subcommand + formats output. Logic lives in the Rust CLI (testable), not the skill prose. Skills are UX over the CLI, nothing more.
