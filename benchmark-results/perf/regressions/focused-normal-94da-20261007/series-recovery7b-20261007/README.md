# Focused Normal follow-up: main versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `dc97f9e73aba7a7ba133df6b5f9d3342577e75df5841f83cffdb65512994381a`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3`; measured engine `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=main elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Main ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.998495 | 0.993220–1.004413 | inconclusive |
| top10 | flagged-case | 1.018718 | 1.006300–1.031213 | reproduced-slowdown |
| join_group_by_city | flagged-case | 1.018092 | 1.000642–1.033558 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 0.997796 | 0.991393–1.005244 | inconclusive |
| join_point | stable-control | 0.986961 | 0.983853–0.998124 | speedup |

Host xbabe1, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 3.50/4.34/5.17. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.

Source-isolation series: Separate recovery7b and WAL8f source changes on the same host; choose lowest load once at series start, retain all five-case ABBA studies and all results. Host selection was fixed at the beginning of the complete series; every execution still records its current load.
