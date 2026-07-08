# Traceability Matrix

Satisfies spec 00 acceptance criterion #1 (every OBJ/CON/KPI maps to a spec rule + test). Keeps the suite honest: an OBJ/CON/KPI with no rule is a hole; a rule with no test is unverified. Update when specs change.

## Objectives → specs
| OBJ | Specs |
|---|---|
| OBJ-1 sovereignty | 07 (provenance), 11 (egress/secrets), 12 U1 (loopback), 00 NFR (escalation=privacy exception) |
| OBJ-2 cost | 05 C14-17, 06 (cheapest route), 08 (delegation), CON-11 |
| OBJ-3 compounding | 02 (semantic), 10 (distill), 13 (ingest), 04 O15 |
| OBJ-4 trust | 05 (fact-check/audit), 02 M7 (status), 10 L4 (supersede) |
| OBJ-5 resilience | 01 (crash-safety), 04 (durable queue), 09 (self-heal) |

## Constraints → rule + test
| CON | Rule | Test |
|---|---|---|
| CON-1 mem 22GB | spec 01 R11/R13 | 01 T2 |
| CON-2 CPU-only | ADR-001, CI portable build | ci.md #4 |
| CON-3 WSL2/no-Docker | ADR-001 | runbook |
| CON-4 Linux fs | spec 01 §5 | 01 T3 |
| CON-5 ≤3 parallel | spec 04 O4 | 04 T3 |
| CON-6 32K ctx | spec 03 I7 | 03 T6, 02 T3 |
| CON-7 egress allowlist | spec 11 S7 | 11 T4 |
| CON-8 self-mod gate | spec 10 L11, 11 S10 | 11 T5, 10 T6 |
| CON-9 secrets env-only | spec 01 §6, 11 S4 | 11 T3, ci.md #8 |
| CON-10 untrusted isolation | spec 07 H3/H4 | 07 T1/T2, 11 T1 |
| CON-11 cost ceiling/depth | spec 05 C15/C16, 04 O6 | 05 T5/T6, 04 T4 |
| CON-12 disk budget | spec 09 H11 | 09 T8 |
| CON-13 secret filter | spec 11 S5 | 11 T2/T3 |

## KPIs → measurement + eval
| KPI | Source | Eval |
|---|---|---|
| KPI-01 local-first ≥75% | ledger route counts | 14 (scheduled) |
| KPI-02 zero bad egress | egress log | 11 T4 |
| KPI-03 self-heal ≥80% | ledger repair outcomes | 09 T1, 14 self_heal |
| KPI-04 ≥6 tok/s | llama.cpp timers | 14 throughput (live) |
| KPI-05 0 mem breaches | RSS sampling | 01 T2 |
| KPI-06 ≥90% fact accuracy | monthly audit | 05 mode 4, 14 fact_audit |
| KPI-07 regret ↓ | bandit curves | 14 router_regret, E9 |
| KPI-08 RAG ≥0.8 top-5 | eval set | 14 rag_qa, 02 T6 |
| KPI-09 ≥70% agent yield | agent_runs | 08 T8 |

## GAPS → owning spec (all 20 land somewhere)
G-01→07/11/13 · G-02→06/16/14 · G-03→03/09 · G-04→05 · G-05→01/04/09 · G-06→05 · G-07→04/08 · G-08→09 · G-09→standards/16 · G-10→02/04/09 · G-11→03 · G-12→05 · G-13→10/14 · G-14→11 · G-15→13 · G-16→06/10 · G-17→07/08 · G-18→12 · G-19→09 · G-20→14/standards

**Holes to watch:** KPI-04/06 depend on live-model evals (not CI-gated). CON-8 depends on spec 14 canary existing before self-mod ships. Reward capture (spec 16) is upstream of KPI-07/09 — build it early (RV-05).
