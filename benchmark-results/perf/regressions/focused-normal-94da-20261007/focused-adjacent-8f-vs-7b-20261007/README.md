# Focused Normal source bisect: WAL window commit versus production parent

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `5fa3c24afacf2b62d4e53aaee412337820760ded90a8b53d9828d0590b7ea580`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4`; measured engine `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4` (identical Cargo/crates inputs). Reference production parent `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3`; this comparison does not measure stable release throughput. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=production parent/B=WAL window commit elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | WAL commit ÷ parent time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 1.014955 | 0.954932–1.090615 | inconclusive |
| top10 | flagged-case | 1.022785 | 0.823841–1.087130 | inconclusive |
| join_group_by_city | flagged-case | 0.994202 | 0.908918–1.048138 | inconclusive |
| point_pk_sql_text | stable-control | 0.990105 | 0.982929–0.999020 | speedup |
| join_point | stable-control | 1.006245 | 1.001350–1.018235 | reproduced-slowdown |

Host xbabe3, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 5.57/6.27/10.98. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
