# Focused Normal source bisect: WAL window commit versus production parent

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `53ccfb5c97665ca74362ab5f9d5e3578ca11ce241b5d57cf379b09a919074251`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4`; measured engine `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4` (identical Cargo/crates inputs). Reference production parent `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3`; this comparison does not measure stable release throughput. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=production parent/B=WAL window commit elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | WAL commit ÷ parent time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 1.024338 | 1.003604–1.033701 | reproduced-slowdown |
| top10 | flagged-case | 1.019243 | 0.932683–1.039519 | inconclusive |
| join_group_by_city | flagged-case | 1.032435 | 1.012521–1.112318 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 1.007006 | 1.002770–1.015573 | reproduced-slowdown |
| join_point | stable-control | 1.011601 | 1.009004–1.015272 | reproduced-slowdown |

Host xbabe1, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 3.28/3.72/4.06. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.

Source-isolation series: Separate recovery7b and WAL8f source changes on the same host; choose lowest load once at series start, retain all five-case ABBA studies and all results. Host selection was fixed at the beginning of the complete series; every execution still records its current load.
