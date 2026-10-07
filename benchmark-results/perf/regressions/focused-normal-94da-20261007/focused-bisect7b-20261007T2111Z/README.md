# Focused Normal follow-up: main versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `2e36bb80e3b71313d62e5b506565f15eee5873ba7ff684d9f1a403a0e6995594`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3`; measured engine `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=main elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Main ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.990442 | 0.974083–0.999579 | speedup |
| top10 | flagged-case | 1.009558 | 0.997271–1.017550 | inconclusive |
| join_group_by_city | flagged-case | 1.094717 | 1.016612–1.294504 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 1.001105 | 0.988418–1.022804 | inconclusive |
| join_point | stable-control | 1.005630 | 1.000755–1.010456 | reproduced-slowdown |

Host xbabe3, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 3.65/4.43/5.78. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
