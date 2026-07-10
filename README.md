# localAI

Self-healing, self-improving, self-learning local AI "Brain" — 32 GB RAM, CPU-only, WSL2. Rust core, Gemma 4 12B local inference, cloud LLM Council for escalation only.

**One loop, three names:** a fact that fails audit applies retroactive negative reward to the route that sourced it; the router unlearns. Self-checking, self-healing, self-improving are the same mechanism.

## Read first

| Doc | What |
|---|---|
| [CLAUDE.md](CLAUDE.md) | project guide, invariants, spec map |
| [PLAN.md](PLAN.md) | master plan, architecture, phase roadmap |
| [specs/](specs/) | 17 numbered specs — **source of truth**, every rule has an ID + test anchor |
| [docs/standards.md](docs/standards.md) | Rust conventions |
| [docs/config.md](docs/config.md) | every config knob |
| [docs/runbook.md](docs/runbook.md) | operations |

## Quickstart (WSL2 Ubuntu 24.04 — mandatory, ADR-001)

```bash
# toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# clone onto the LINUX filesystem — never /mnt/c (CON-4: 9P kills SQLite)
git clone https://github.com/antarikshkarmakar/localAI ~/localAI
cd ~/localAI

# native build (CPU-only box, CON-2)
RUSTFLAGS="-C target-cpu=native" cargo build --release

# tests
cargo test --workspace
```

Secrets: environment only (`.env.example` has key names). `config.toml` rejects key-like values at load.

## Status

Phase 1 (workspace, schema, queue, ledger, config) — in progress. Phases: [PLAN.md §13](PLAN.md).

## License

MIT
