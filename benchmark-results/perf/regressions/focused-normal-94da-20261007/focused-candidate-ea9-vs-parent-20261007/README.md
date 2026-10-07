# Focused Normal candidate experiment: scratch-window candidate versus production parent

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `9fce310e1c1b39571a84097eb73b243c60f5ccaab51730ecd1cf91f598470487`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `ea9dd55e8d63c6bbe8190e4318269b7efa05c776`; measured engine `ea9dd55e8d63c6bbe8190e4318269b7efa05c776` (identical Cargo/crates inputs). Reference production parent `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4`; this comparison does not measure stable release throughput. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=production parent/B=scratch-window candidate elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Candidate ÷ parent time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 1.003334 | 1.001967–1.007929 | reproduced-slowdown |
| top10 | flagged-case | 1.000508 | 0.984707–1.007384 | inconclusive |
| join_group_by_city | flagged-case | 1.018257 | 1.005295–1.029623 | reproduced-slowdown |
| point_pk_sql_text | stable-control | 1.004779 | 0.998648–1.013492 | inconclusive |
| join_point | stable-control | 1.001781 | 0.999321–1.005393 | inconclusive |

Host xbabe1, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 3.46/3.88/4.16. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
