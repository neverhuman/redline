# Focused Normal follow-up: main versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `20df81485c5c860d3ed7ac1f32bdee8c838b7ce5ccf843e3311ad841c7395f95`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `94da3b131d6d50009d2d24e026b9010681e1f04d`; measured engine `bac7e9186feaa21824369c97767c391100ad1c37` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=main elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Main ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 1.025219 | 1.012034–1.167509 | reproduced-slowdown |
| top10 | flagged-case | 1.041350 | 1.024017–1.084762 | reproduced-slowdown |
| join_group_by_city | flagged-case | 1.020101 | 1.007548–1.050007 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 0.994916 | 0.990243–0.999747 | speedup |
| join_point | stable-control | 0.990516 | 0.986737–0.993493 | speedup |

Host xbabe3, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 5.61/8.45/10.64. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
