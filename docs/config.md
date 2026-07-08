# Configuration Reference

Every `config.toml` key, its default, env override (`LOCALAI_` prefix), and the constraint it enforces. Single registry — specs scatter these; this is the source of truth. Secrets are NEVER here (env only, CON-9).

## [system]
| Key | Default | Env | Enforces |
|---|---|---|---|
| `data_dir` | `~/brain` | `LOCALAI_DATA_DIR` | CON-4 — rejected if under `/mnt/*` (spec 01 §5) |
| `mem_ceiling_gb` | 22 | `LOCALAI_MEM_CEILING_GB` | CON-1 (spec 01 R11) |
| `mem_soft_gb` | 19 | — | soft watermark, stop new jobs (spec 01 R13) |
| `mem_hard_gb` | 21 | — | hard watermark, kill worker (R13) |
| `mem_critical_gb` | 22 | — | kill model, degraded mode (R13) |
| `mem_sample_secs` | 5 | — | MemoryGuard poll (spec 01 §4) |
| `disk_soft_gb` / `disk_hard_gb` | 20 / 5 free | — | CON-12, retention/stop (spec 09 H11) |

## [inference]
| Key | Default | Enforces |
|---|---|---|
| `primary_model` | `gemma4-12b-Q4_K_M` | ADR-003 |
| `fast_model` | `gemma4-e4b` | ADR-003, self-consistency/background |
| `resident` | `fast` | E4B resident, 12B on-demand (REVIEW RV-04) |
| `ctx_size` | 32768 | CON-6 (spec 03 I7) |
| `mtp_draft_n_max` | 2 | spec 03 §1, tune 1–6 |
| `gen_reserve_tokens` | 4096 | spec 02 §2 |
| `self_consistency_k` | 3 | spec 06; opt-in high-stakes only (RV-03) |
| `gen_timeout_base_secs` | 120 | spec 03 I14 |
| `server_port` | (auto) | spec 01 R3 |

## [embeddings]
| Key | Default | Enforces |
|---|---|---|
| `model` | fastembed MiniLM-384 | spec 02 M9 (RV-10 flags weak-for-code) |
| `dim` | 384 | spec 02 |
| `quant` | int8 | spec 02 M9 |

## [orchestration]
| Key | Default | Enforces |
|---|---|---|
| `max_parallel_jobs` | 3 | CON-5 (spec 04 O4) |
| `job_max_attempts` | 3 | spec 04 O13 |
| `job_lease_secs` | 600 | crash detection (spec 04 O3) |
| `spawn_depth_max` | 2 | G-07 (spec 04 O6) |
| `worker_mem_limit_gb` | 1.5 | spec 01 R14 / 04 O7 |
| `starvation_age_secs` | 3600 | priority aging (spec 04 O5) |

## [council]
| Key | Default | Enforces |
|---|---|---|
| `providers` | `[anthropic, openai, gemini]` | spec 05 |
| `models.*` | (config) | liveness-pinged at boot (spec 05 C2) |
| `daily_usd_ceiling` | 5.00 | CON-11 (spec 05 C15) |
| `monthly_usd_ceiling` | 50.00 | CON-11 |
| `fact_check_tier` | cheap | spec 05 C17 |
| `sovereign_mode` | false | council disabled for sensitive sessions (REVIEW RV-06) |

## [router]
| Key | Default | Enforces |
|---|---|---|
| `bandit` | thompson | ADR-005 |
| `warmup_decisions` | 500 | KPI withhold until warm (spec 06 R12, G-16) |
| `reward_weights` | `{w1..w5}` versioned | spec 06 R10 (recomputable) |
| `priors.*` | (table) | cold-start routes (spec 06 R12) |

## [learning]
| Key | Default | Enforces |
|---|---|---|
| `reward_hold_window_hrs` | 24 | spec 06 R9 / 16 RS4 (seq-ordered) |
| `prompt_ab_min_sample` | (tbd) | no promotion on noise (spec 10 L9) |
| `canary_kpi_regress_pct` | 5 | auto-reject/rollback (spec 10 L16) |

## [memory]
| Key | Default | Enforces |
|---|---|---|
| `hot_cache_mb` | 512 | spec 02 M13 eviction |
| `chunk_tokens` / `chunk_overlap` | 350 / 0.15 | spec 02 M10 / 13 D14 |
| `episode_hot_days` | 90 | spec 02 M6 |

## [ingestion]
| Key | Default | Enforces |
|---|---|---|
| `scrape_allowlist` | `[]` | CON-7 (spec 13 D1) |
| `per_domain_rate` | (config) | politeness (spec 13 D2) |
| `max_fetch_mb` | 10 | poison/zip-bomb cap (spec 13 D16) |
| `headless_browser` | false | JS-render gate (spec 13 D7) |

## [retention]
| Key | Default | Enforces |
|---|---|---|
| `artifact_verbose_days` | 14 | prune logs, keep brief/handoff/diff (spec 08 A14) |
| `backup_cron` | daily | DB backup (REVIEW RV-07) |
| `kb_git` | true | `kb/` under git (REVIEW RV-07) |
| `index_max_lines` | 300 | MEMORY.md compress-alert (spec 02 M6b, antarikshSkills B3) |
| `memory_archive_kb` | 100 | archive daily logs >14d when `kb/` exceeds (M6b) |
| `daily_log_hot_days` | 14 | keep recent logs hot, archive older (M6b) |
| `skill_obs_max_lines` | 150 | archive ACTIONED/DECLINED >30d (M6b) |
| `skill_obs_archive_days` | 30 | age-out closed observations (spec 10 L9b) |

> Any new config knob added by a spec MUST land here in the same PR (avoids the config-drift failure mode).
