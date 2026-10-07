# Focused Normal candidate experiment: checkpoint-open load-order candidate versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `3d95cb6f1a45285c893c9eebbec8b963dc8b659acaddfd0a0d1aa7c13a3665ff`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `4f9cb79224bd3d896188c48bdc0f22cdea1a0d39`; measured engine `4f9cb79224bd3d896188c48bdc0f22cdea1a0d39` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=checkpoint-open load-order candidate elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Candidate ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.988039 | 0.981336–0.993775 | speedup |
| top10 | flagged-case | 1.027876 | 1.018512–1.065782 | reproduced-slowdown |
| join_group_by_city | flagged-case | 1.020442 | 1.009937–1.033296 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 0.996151 | 0.990945–0.999550 | speedup |
| join_point | stable-control | 1.006727 | 0.990911–1.015696 | inconclusive |

Host xbabe1, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 5.15/5.68/13.56. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
