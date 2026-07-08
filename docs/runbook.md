# Runbook — Operations

Ops procedures for the local Brain. WSL2 (Ubuntu 24.04), bare metal (ADR-001).

## Prerequisites
- WSL2 with **systemd enabled** — `/etc/wsl.conf`:
  ```ini
  [boot]
  systemd=true
  ```
  Then `wsl --shutdown` from Windows, reopen. (REVIEW RV-09 — systemd is opt-in in WSL2.)
  **No-systemd fallback:** run the watchdog as a backgrounded script launched from `~/.profile`, or a Windows Task Scheduler job that invokes `wsl -e`.
- Rust toolchain (pinned via `rust-toolchain.toml`), `llama.cpp` built native (`target-cpu=native`).
- Models in `data_dir/models/` (Linux fs, NOT `/mnt/c` — CON-4).

## Start / stop
- Start: `systemctl --user start localai` (watchdog → brain → llama-server, spec 01 §5).
- Stop: `systemctl --user stop localai` (SIGTERM → grace 30s → children down, spec 01 §5).
- Status: `systemctl --user status localai`; live state in UI `127.0.0.1:4321`.

## Watchdog
- `localai-watchdog` restarts Brain on missed heartbeat (spec 09 H9). It only restarts — no logic (small TCB).
- Brain restart auto-recovers: re-queues orphaned `running` jobs, reconciles OKF↔DB + ledger spill, resumes (spec 09 H10).

## Model swap
- Hot-swap (no Brain restart): `localai model swap <id>` — drains queue to new llama-server, SIGTERMs old (spec 03 I5). Logged as `model_swap`.
- Registry: `localai model list` (shows RSS estimates vs budget, spec 03 I4).

## DB rebuild (disaster recovery)
- `kb/` is ground truth (spec 02 M1) and git-tracked (RV-07). DB is a rebuildable index:
  1. `localai rebuild-index` — re-scans `kb/`, rebuilds `okf_documents` + vectors (spec 02 T1 guarantees identical retrieval).
  2. Ledger + procedural tables restore from the daily backup (`data_dir/backups/`, RV-07) and/or `ledger.spill.jsonl` replay (G-05).
- Full reset: restore latest `VACUUM INTO` backup → `rebuild-index` → replay spill.

## Degraded modes (spec 09 H12 — all show a UI banner, none silent)
| Trigger | Mode | Recover |
|---|---|---|
| model `/health` fail | council-only answers | restart llama-server |
| embedding version mismatch/corruption | FTS-only retrieval | finish re-embed job |
| cloud budget exhausted | local-only | budget reset / raise ceiling |
| disk hard threshold | read-only, no new jobs | retention sweep / free space |

## Maintenance jobs (scheduled, spec 04 O15)
Nightly: episode rollup, OKF↔DB reconcile, WAL checkpoint(TRUNCATE), disk retention sweep, DB backup.
Monthly: fact calibration audit (spec 05 mode 4 → KPI-06, feeds retroactive reward).

## Common incidents
- **WAL file huge:** long read holding checkpoint — kill it; scheduled `wal_checkpoint(TRUNCATE)` (spec 09 H7).
- **RAM breach alerts:** check resident model + workers; likely 12B + heavy worker co-resident (RV-04) — enable `resident=fast`.
- **Council calls failing:** provider breaker open (spec 05 C3) — check `localai council status`; verify model ids live (C2).
- **Secret flagged:** a `secret_flag` event means SecretFilter caught something outbound/persisted (spec 11 S5) — audit whether a real secret nearly leaked.
