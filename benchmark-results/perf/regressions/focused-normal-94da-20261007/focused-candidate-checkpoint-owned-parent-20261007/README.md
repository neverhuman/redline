# Focused Normal candidate experiment: bounded owned-decode candidate versus production parent

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `e6ba62e0aacc73d7944a92852d01752609553f70940e2475df28878d3e7ea512`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `ac96a7150a5ad4c244605ec09124518d5f016dae`; measured engine `ac96a7150a5ad4c244605ec09124518d5f016dae` (identical Cargo/crates inputs). Reference production parent `8f880b2162413dfff2e7d5ee0a5dc4a979a851e4`; this comparison does not measure stable release throughput. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=production parent/B=bounded owned-decode candidate elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Candidate ÷ parent time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.759813 | 0.567379–0.918799 | speedup |
| top10 | flagged-case | 0.998945 | 0.943907–1.031615 | inconclusive |
| join_group_by_city | flagged-case | 0.997455 | 0.993805–1.001916 | inconclusive |
| point_pk_sql_text | stable-control | 0.996013 | 0.990286–0.999673 | speedup |
| join_point | stable-control | 0.998489 | 0.986297–1.005835 | inconclusive |

Host xbabe3, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 3.47/4.66/5.93. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
