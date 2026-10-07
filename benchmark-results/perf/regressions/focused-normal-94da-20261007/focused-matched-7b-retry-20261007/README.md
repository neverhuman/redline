# Focused Normal follow-up: main versus v5.1.1

Generated from [plan](plan.json), [raw records](records.jsonl), [per-execution host telemetry](host-samples.jsonl) and [receipt](receipt.json). Receipt SHA-256: `e991c32d9f982475bb25459a8b4ce8f086b5f418272df37be8eabcb2fb44e0fe`. This diagnostic does not replace publishable quiet-host release throughput.

Compared commit `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3`; measured engine `7bdf4c8c55d51e0b9895d53299bd0ae442b67fe3` (identical Cargo/crates inputs). Measured release `af4fc74cc3280582c4e6bd0583fe503c83118c48` has identical Cargo/crates inputs to stable `9277455d5ad008252053a81d18add39b8cdc8f7b`. Both frozen production builds use harness `bb1434a80ca0390401eb628d2785e8387c5f346e`, identical flags, original 20,000-row images, full operations and result digests.

Only the three predeclared flags and two stable controls ran. Each case has 12 ABBA blocks (24 measured executions per version) and one retained warmup block. The median uses sqrt(B1×B2/(A1×A2)), with A=release/B=main elapsed time per equal operation. The percentile95% bootstrap resamples whole blocks10,000 times with recorded seeds. A slowdown reproduces only if its entire interval is above1.0. An interval below1.0 is a speedup; crossing1.0 is inconclusive, not equivalence. Intervals are per case without multiplicity correction.

| Case | Role | Main ÷ release time | Bootstrap95%CI | Finding |
|---|---|---:|---:|---|
| point_pk_prepared | flagged-case | 0.999323 | 0.990769–1.016144 | inconclusive |
| top10 | flagged-case | 1.018273 | 0.998685–1.040630 | inconclusive |
| join_group_by_city | flagged-case | 1.002277 | 0.974503–1.016189 | inconclusive |
| point_pk_sql_text | stable-control | 0.993714 | 0.969591–1.012759 | inconclusive |
| join_point | stable-control | 0.998080 | 0.995257–1.008147 | inconclusive |

Host xbabe1, taskset2-3, nice19, idle I/O. These cores are pinned for both builds, not kernel-isolated or exclusive; SMT sibling activity and runner presence are recorded. No peer services, priorities or affinities were changed. Load1 min/median/max: 4.82/6.08/8.32. Records: 260 passed / 0 failed / 0 skipped, including20 retained warmups.
