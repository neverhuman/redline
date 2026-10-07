# Focused Normal follow-up: main versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `ee845d2f37442786ee9147d9fa31dad7ce89d5d03a36a6461b5ac4c6edebfe22`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4`; measured engine `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=main elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Main ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 1.091467 | 1.021061–1.200435 | reproduced-slowdown |
| top10 | flagged-case | 1.138556 | 1.049523–1.526318 | reproduced-slowdown |
| join_group_by_city | flagged-case | 1.087037 | 1.037750–1.139230 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 1.006077 | 0.997225–1.018949 | inconclusive |
| join_point | stable-control | 1.014493 | 1.005105–1.016241 | reproduced-slowdown |

Host xbabe1, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 4.24/9.29/11.58. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.

Source-isolation series: Separate recovery7b and WAL8f source changes on the same host; choose lowest load once at series start, retain all five-case ABBA studies and all results. Host selection was fixed at the beginning of the complete series; every execution still records its current load.
