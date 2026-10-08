# Tighter Normal Top 10 and grouped-join comparison

Generated from immutable complete-session receipts. Main 3e8b2d5b6f13a4440c10ad5c56bff73d3d967538 versus exact v5.1.1 9277455d5ad008252053a81d18add39b8cdc8f7b. Both package-only release builds use harness bb1434a80ca0390401eb628d2785e8387c5f346e, identical toolchain/profile/flags and original full-work 20,000-row case operations, digests and settings.

Two predeclared sessions each run 128 complete ABBA blocks per case plus one retained warmup. Cases are interleaved inside each block; the second session reverses primary case order. Ratios are main/release query elapsed time per equal operation; lower is better. The median is over 256 ABBA ratios. A 50000-draw seeded circular moving-block bootstrap preserves eight adjacent complete blocks and equal session strata. Per-case 95% intervals and 98.75% primary decision intervals are shown; the latter use a Bonferroni family size of 4. The control uses its 95% interval. A reproduced primary direction requires its decision interval and both session 95% intervals wholly on the same side of 1.0. Crossing remains inconclusive, not equivalence. Bootstrap coverage is an estimate under recorded conditions; it does not eliminate shared-host confounding. This single-core follow-up was chosen after the first study stayed inconclusive; it uses a four-comparison Bonferroni interval as a conservative diagnostic, without claiming fixed-sample familywise coverage for the adaptive investigation. This remains an exploratory diagnostic, with no further adaptive extension of its declared sample size.

| Case | Role | Main / release | 95% interval | Decision interval | Finding | Upper bound below 1.05? |
|---|---|---:|---:|---:|---|---|
| top10 | primary | 1.255862 | 1.096097–1.274687 | 1.062790–1.278604 | reproduced-slowdown | no |
| join_group_by_city | primary | 1.260414 | 1.106840–1.277681 | 1.066784–1.283821 | reproduced-slowdown | no |
| point_pk_sql_text | control | 1.039218 | 1.021986–1.047905 | 1.021986–1.047905 | reproduced-slowdown | yes |

| Case | Session | Main / release | Session 95% interval |
|---|---:|---:|---:|
| top10 | 1 | 1.278612 | 1.070546–1.298816 |
| top10 | 2 | 1.250843 | 1.039568–1.266274 |
| join_group_by_city | 1 | 1.199381 | 1.061552–1.303584 |
| join_group_by_city | 2 | 1.264257 | 1.083964–1.274725 |
| point_pk_sql_text | 1 | 1.038590 | 1.010341–1.053201 |
| point_pk_sql_text | 2 | 1.039218 | 1.022125–1.045768 |

Records: 3096 passed / 0 failed / 0 skipped, including 24 retained warmups. Host xbabe3, pinned 23, nice19 and idle I/O; cores are not exclusive or kernel-isolated. Load1 min/median/max 4.49/7.16/27.83. Selected-core/SMT counters, actual child scheduling, CI worker presence and per-execution loads remain in raw telemetry. No samples were dropped for load or timing.

Session receipt SHA-256: 278acf0a0a1a610897ad956725008bd37eb43a2c135123ffd9aedbcffbd0f60a, 042a438207464d7936394d2b38571d3a9840b59b336938918b316ec3eaec6b64. These query-timer diagnostics do not replace published absolute release throughput, attribute a unique cause, or qualify a future release.
