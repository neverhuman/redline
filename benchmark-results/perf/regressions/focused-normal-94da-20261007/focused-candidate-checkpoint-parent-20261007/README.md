# Focused Normal candidate experiment: checkpoint-open load-order candidate versus production parent

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `4c6d538b921c0679052f015792cd21691731f52c015df73c3c975288029846d6`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `4f9cb79224bd3d896188c48bdc0f22cdea1a0d39`; measured engine `4f9cb79224bd3d896188c48bdc0f22cdea1a0d39` (identical Cargo/crates inputs). Reference production parent `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4`; this comparison does not measure stable release throughput. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=production parent/B=checkpoint-open load-order candidate elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Candidate ÷ parent time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.992846 | 0.977384–1.013540 | inconclusive |
| top10 | flagged-case | 0.993242 | 0.982403–1.026352 | inconclusive |
| join_group_by_city | flagged-case | 1.002984 | 0.907581–1.050345 | inconclusive |
| point_pk_sql_text | stable-control | 0.999588 | 0.986749–1.011015 | inconclusive |
| join_point | stable-control | 1.005179 | 0.995437–1.012037 | inconclusive |

Host xbabe3, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 5.66/6.65/9.52. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
