# Tighter Normal Top 10 and grouped-join comparison

Generated from immutable complete-session receipts. Main 3e8b2d5b6f13a4440c10ad5c56bff73d3d967538 versus exact v5.1.1 9277455d5ad008252053a81d18add39b8cdc8f7b. Both package-only release builds use harness bb1434a80ca0390401eb628d2785e8387c5f346e, identical toolchain/profile/flags and original full-work 20,000-row case operations, digests and settings.

Two predeclared sessions each run 64 complete ABBA blocks per case plus one retained warmup. Cases are interleaved inside each block; the second session reverses primary case order. Ratios are main/release query elapsed time per equal operation; lower is better. The median is over 128 ABBA ratios. A 50,000-draw seeded circular moving-block bootstrap preserves eight adjacent complete blocks and equal session strata. Per-case 95% intervals and 97.5% primary intervals are shown; the latter use a Bonferroni correction for two primary cases. A reproduced direction requires the adjusted pooled interval and both session 95% intervals wholly on the same side of 1.0. Crossing remains inconclusive, not equivalence. Bootstrap coverage is an estimate under recorded conditions; it does not eliminate shared-host confounding.

| Case | Role | Main / release | 95% interval | 97.5% interval | Finding |
|---|---|---:|---:|---:|---|
| top10 | primary | 1.009179 | 1.001115–1.049643 | 0.998003–1.054555 | inconclusive |
| join_group_by_city | primary | 1.003248 | 0.990996–1.016015 | 0.989215–1.018074 | inconclusive |
| point_pk_sql_text | control | 1.003212 | 1.000305–1.008660 | 1.000038–1.009598 | inconclusive |

Records: 1560 passed / 0 failed / 0 skipped, including 24 retained warmups. Host xbabe3, pinned 32,46, nice19 and idle I/O; cores are not exclusive or kernel-isolated. Load1 min/median/max 7.31/25.10/55.64. Selected-core/SMT counters, actual child scheduling, CI worker presence and per-execution loads remain in raw telemetry. No samples were dropped for load or timing.

Session receipt SHA-256: 96eacd81308478915aa424b1585686e3c8fdecd78118b8de9f3d07b06435a12b, af791942a788fe8ca46231394864f5f3adb79579332b60bf23ec47fa627c419f. These query-timer diagnostics do not replace published absolute release throughput, attribute a unique cause, or qualify a future release.
