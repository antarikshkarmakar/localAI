# schemas/ — Machine-readable contracts

JSON Schema (draft 2020-12) for every cross-boundary contract the specs describe in prose. These are **normative**: code validates against them, tests assert them, drift is a build failure.

Why: prose specs drift from code. A versioned schema is the single source of truth for a wire/storage format — the tracker-drift failure mode (see MEMORY / GAPS) applied to data contracts.

| Schema | Contract | Spec |
|---|---|---|
| `worker-result.schema.json` | worker stdout JSON | 04 O8 |
| `event-payload.schema.json` | ledger `events.payload` per `kind` | 02 §3, 16 |
| `brief.schema.json` | `BRIEF.md` YAML frontmatter | 08 §4 |
| `handoff.schema.json` | `HANDOFF.md` YAML frontmatter | 08 §5 |
| `hook-verdict.schema.json` | hook stdin/stdout contract | 07 §4 |
| `okf-frontmatter.schema.json` | OKF `.md` YAML frontmatter | 02 §4.1 |

## Rules
- **Every schema has a `version` field.** Bump on any change; consumers check it.
- **`additionalProperties: false`** on closed contracts (wire formats) — unknown keys are errors, not silently dropped.
- Schemas live here, not inline in code, so specs / tests / multiple crates share one definition.
- A `$comment` on each field points back to the spec rule it enforces.
