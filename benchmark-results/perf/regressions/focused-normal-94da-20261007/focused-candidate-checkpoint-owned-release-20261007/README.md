# Focused Normal candidate experiment: bounded owned-decode candidate versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `2d429ed5c2209a09258316a296ece82d5ce1fb73cc37d98acf2d7476c84971d1`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `ac96a7150a5ad4c244605ec09124518d5f016dae`; measured engine `ac96a7150a5ad4c244605ec09124518d5f016dae` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=bounded owned-decode candidate elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Candidate ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.981499 | 0.979517–0.983217 | speedup |
| top10 | flagged-case | 0.978825 | 0.967951–0.984239 | speedup |
| join_group_by_city | flagged-case | 0.986122 | 0.975440–0.999915 | speedup |
| point_pk_sql_text | stable-control | 0.998780 | 0.992745–1.012575 | inconclusive |
| join_point | stable-control | 0.992635 | 0.977493–1.012211 | inconclusive |

Host xbabe3, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 5.10/5.77/27.93. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
