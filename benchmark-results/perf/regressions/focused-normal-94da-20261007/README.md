# Focused Normal regression investigation

Generated from every retained full-work receipt. 16 complete studies; 4160 passed / 0 failed / 0 skipped executions, including retained warmups. These diagnostics do not replace publishable release throughput or establish a regression-free future release.

The owner requested only three flags and two stable controls, with the reproduction boundary at 1.0. Every study uses the same five cases, 20,000 rows, full operations/digests/settings, 12 ABBA blocks and one warmup block, pinned CPUs2–3, nice19 and idle I/O. The seeded percentile95% bootstrap resamples whole blocks. Intervals are per case without multiplicity correction; crossing1.0 is inconclusive. Hosts were naturally available; cores were pinned but neither kernel-isolated nor exclusive. Selected-core/SMT counters and runner presence are in each host log. The closing three-study series chose the lowest-load host once at its beginning and kept that host; other studies made a fresh lowest-load selection.

| Study | Reference → measured engine | Host | Prepared point ratio [CI] | Top10 ratio [CI] | Grouped join ratio [CI] | SQL-text control [CI] | Join-point control [CI] |
|---|---|---|---:|---:|---:|---:|---:|
| [focused-adjacent-8f-vs-7b-20261007](focused-adjacent-8f-vs-7b-20261007/README.md) | 7bdf4c8c → 8f880b21 | xbabe3 | 1.014955 [0.954932, 1.090615] | 1.022785 [0.823841, 1.087130] | 0.994202 [0.908918, 1.048138] | 0.990105 [0.982929, 0.999020] | 1.006245 [1.001350, 1.018235] |
| [focused-baseline141-20261007](focused-baseline141-20261007/README.md) | af4fc74c → 141247e9 | xbabe3 | 1.013912 [0.999827, 1.030167] | 1.029277 [1.025808, 1.043362] | 1.005911 [1.001142, 1.007671] | 0.998520 [0.992799, 1.009024] | 0.987684 [0.977301, 0.997308] |
| [focused-bisect-e9-20261007](focused-bisect-e9-20261007/README.md) | af4fc74c → e9afed9e | xbabe3 | 1.008976 [1.002995, 1.022757] | 1.045558 [1.035535, 1.049869] | 1.005745 [0.999537, 1.013301] | 1.029037 [1.018480, 1.036798] | 0.985613 [0.979116, 0.990091] |
| [focused-bisect7b-20261007T2111Z](focused-bisect7b-20261007T2111Z/README.md) | af4fc74c → 7bdf4c8c | xbabe3 | 0.990442 [0.974083, 0.999579] | 1.009558 [0.997271, 1.017550] | 1.094717 [1.016612, 1.294504] | 1.001105 [0.988418, 1.022804] | 1.005630 [1.000755, 1.010456] |
| [focused-candidate-ea9-vs-parent-20261007](focused-candidate-ea9-vs-parent-20261007/README.md) | 8f880b21 → ea9dd55e | xbabe1 | 1.003334 [1.001967, 1.007929] | 1.000508 [0.984707, 1.007384] | 1.018257 [1.005295, 1.029623] | 1.004779 [0.998648, 1.013492] | 1.001781 [0.999321, 1.005393] |
| [focused-matched-7b-retry-20261007](focused-matched-7b-retry-20261007/README.md) | af4fc74c → 7bdf4c8c | xbabe1 | 0.999323 [0.990769, 1.016144] | 1.018273 [0.998685, 1.040630] | 1.002277 [0.974503, 1.016189] | 0.993714 [0.969591, 1.012759] | 0.998080 [0.995257, 1.008147] |
| [focused-matched-e9-20261007](focused-matched-e9-20261007/README.md) | af4fc74c → e9afed9e | xbabe3 | 0.997446 [0.986407, 1.039988] | 0.994615 [0.978210, 1.031096] | 0.980969 [0.944665, 1.010826] | 1.017656 [0.999334, 1.076187] | 1.005031 [0.995214, 1.015334] |
| [focused-matched-main-20261007](focused-matched-main-20261007/README.md) | af4fc74c → 8f880b21 | xbabe3 | 1.025590 [1.023003, 1.030950] | 1.094217 [1.059895, 1.110205] | 1.618624 [1.239674, 1.934607] | 1.007313 [0.988326, 1.014022] | 1.005979 [0.996458, 1.011937] |
| [focused-normal-94da-20261007T2100Z](focused-normal-94da-20261007T2100Z/README.md) | af4fc74c → bac7e918 | xbabe3 | 1.025219 [1.012034, 1.167509] | 1.041350 [1.024017, 1.084762] | 1.020101 [1.007548, 1.050007] | 0.994916 [0.990243, 0.999747] | 0.990516 [0.986737, 0.993493] |
| [series-main8f-20261007](series-main8f-20261007/README.md) | af4fc74c → 8f880b21 | xbabe1 | 1.091467 [1.021061, 1.200435] | 1.138556 [1.049523, 1.526318] | 1.087037 [1.037750, 1.139230] | 1.006077 [0.997225, 1.018949] | 1.014493 [1.005105, 1.016241] |
| [series-recovery7b-20261007](series-recovery7b-20261007/README.md) | af4fc74c → 7bdf4c8c | xbabe1 | 0.998495 [0.993220, 1.004413] | 1.018718 [1.006300, 1.031213] | 1.018092 [1.000642, 1.033558] | 0.997796 [0.991393, 1.005244] | 0.986961 [0.983853, 0.998124] |
| [series-wal8f-20261007](series-wal8f-20261007/README.md) | 7bdf4c8c → 8f880b21 | xbabe1 | 1.024338 [1.003604, 1.033701] | 1.019243 [0.932683, 1.039519] | 1.032435 [1.012521, 1.112318] | 1.007006 [1.002770, 1.015573] | 1.011601 [1.009004, 1.015272] |
| [focused-candidate-checkpoint-parent-20261007](focused-candidate-checkpoint-parent-20261007/README.md) | 8f880b21 → 4f9cb792 | xbabe3 | 0.992846 [0.977384, 1.013540] | 0.993242 [0.982403, 1.026352] | 1.002984 [0.907581, 1.050345] | 0.999588 [0.986749, 1.011015] | 1.005179 [0.995437, 1.012037] |
| [focused-candidate-checkpoint-release-20261007](focused-candidate-checkpoint-release-20261007/README.md) | af4fc74c → 4f9cb792 | xbabe1 | 0.988039 [0.981336, 0.993775] | 1.027876 [1.018512, 1.065782] | 1.020442 [1.009937, 1.033296] | 0.996151 [0.990945, 0.999550] | 1.006727 [0.990911, 1.015696] |
| [focused-candidate-checkpoint-owned-parent-20261007](focused-candidate-checkpoint-owned-parent-20261007/README.md) | 8f880b21 → ac96a715 | xbabe3 | 0.759813 [0.567379, 0.918799] | 0.998945 [0.943907, 1.031615] | 0.997455 [0.993805, 1.001916] | 0.996013 [0.990286, 0.999673] | 0.998489 [0.986297, 1.005835] |
| [focused-candidate-checkpoint-owned-release-20261007](focused-candidate-checkpoint-owned-release-20261007/README.md) | af4fc74c → ac96a715 | xbabe3 | 0.981499 [0.979517, 0.983217] | 0.978825 [0.967951, 0.984239] | 0.986122 [0.975440, 0.999915] | 0.998780 [0.992745, 1.012575] | 0.992635 [0.977493, 1.012211] |

Ratios are measured/reference elapsed time per equal operation. The eight-character hashes above are shorthand; each linked report records full commits, binary hashes and exact reference roles. The scratch-window candidate is a rejected experiment, not a merged fix. Preserve positive, inconclusive and contradictory comparisons together.

## Candidate scope and process cost

The final candidate retains the 1 MiB WAL read window, adds an owned encoded buffer for ordinary records up to 512 KiB, and defers directory construction when no WAL remains after the checkpoint. The first change adds one allocation, copy and free per ordinary record; oversized records still borrow the window. It does not restore the complete historical allocation sequence or halve resident memory. Before/after data support prepared-lookup improvement; the other flagged intervals cross 1.0 against the parent. The separate matched-release comparison places all three flagged intervals below 1.0, with controls inconclusive. The grouped-join upper bound is very close to 1.0, and parent prepared timings contain reference outliers; the apparent parent gain is not a stable general percentage. Candidate selection was exploratory and adaptive. This is evidence for a mitigation under the recorded conditions, not unique causal attribution, equivalence or a regression-free future release.

Child-lifetime diagnostics below use the same retained ABBA blocks and bootstrap seeds. Millisecond wall-clock timestamps include process launch, image copying, open/recovery, correctness checks, queries and output. They do not measure pure open/recovery cost and must not be substituted for the query timer above.

| Reference | Case | Reference median ms | Candidate median ms | Candidate/reference child-lifetime ratio [95% CI] |
|---|---|---:|---:|---:|
| Parent | point_pk_prepared | 1004.5 | 897.5 | 0.982912 [0.775211, 1.120662] |
| Parent | top10 | 194 | 195.5 | 0.997479 [0.970843, 1.022458] |
| Parent | join_group_by_city | 123 | 123 | 1.002062 [0.997963, 1.011743] |
| Parent | point_pk_sql_text | 386 | 385.5 | 0.997432 [0.992356, 1.001298] |
| Parent | join_point | 2552 | 2545 | 0.991528 [0.979404, 1.004939] |
| Release | point_pk_prepared | 530 | 477 | 0.899920 [0.897382, 0.901655] |
| Release | top10 | 194 | 146 | 0.752570 [0.749388, 0.756165] |
| Release | join_group_by_city | 175.5 | 129 | 0.734157 [0.726128, 0.739879] |
| Release | point_pk_sql_text | 440 | 393.5 | 0.896923 [0.891312, 0.905065] |
| Release | join_point | 3194.5 | 3033.5 | 0.999298 [0.911720, 1.126970] |

## Source and artifact limits

[Commit inventory](commit-inventory.json) covers 26 commits and 2018 changed-path entries, with hypotheses recorded before bisect. e9 changes range routing/shared rowid detection; 7b moves directory construction before heap redo; 8f changes WAL scan buffering. No direct TopK or grouped-join algorithm change was found. Queries begin after opening, so startup allocation/cache history remains a competing explanation. [Binary provenance](binary-provenance-analysis.json) shows that historical and rebuilt AF4 executables have identical normalized instruction streams but different linked data/addresses. That is not semantic or timing equivalence and does not prove a layout cause. Source comparisons use the matched rebuilt AF4 reference.

[Investigation manifest](investigation.json) binds supplemental logs, inventory, source experiments, traced diagnostics and archive custody receipts by SHA-256. Traced executions are separate diagnostics and never timing acceptance; untraced studies above retain their original raw samples.

## Receipt hashes and observed load

- focused-adjacent-8f-vs-7b-20261007: receipt SHA-256 `5fa3c24afacf2b62d4e53aaee412337820760ded90a8b53d9828d0590b7ea580`; load min/median/max 5.57/6.27/10.98.
- focused-baseline141-20261007: receipt SHA-256 `df326ad0420ff22b35e8ac7c3d625f17cdfcd77773057c14e623f26515d305cc`; load min/median/max 4.83/10.39/12.08.
- focused-bisect-e9-20261007: receipt SHA-256 `e48ca0020d51510c0d9d164275e0ea42b58ce19a1769858ef0401e645dca4b52`; load min/median/max 5.06/10.55/12.15.
- focused-bisect7b-20261007T2111Z: receipt SHA-256 `2e36bb80e3b71313d62e5b506565f15eee5873ba7ff684d9f1a403a0e6995594`; load min/median/max 3.65/4.43/5.78.
- focused-candidate-ea9-vs-parent-20261007: receipt SHA-256 `9fce310e1c1b39571a84097eb73b243c60f5ccaab51730ecd1cf91f598470487`; load min/median/max 3.46/3.88/4.16.
- focused-matched-7b-retry-20261007: receipt SHA-256 `e991c32d9f982475bb25459a8b4ce8f086b5f418272df37be8eabcb2fb44e0fe`; load min/median/max 4.82/6.08/8.32.
- focused-matched-e9-20261007: receipt SHA-256 `f057ef028836de9559901b9eb05e539d746f1c8c6d62113f60de8679ade98bdf`; load min/median/max 4.95/6.23/6.57.
- focused-matched-main-20261007: receipt SHA-256 `1499adeca3740be27e5715b90513ee44a0ff4571881125cefa63224c00b67f82`; load min/median/max 5.04/5.49/6.39.
- focused-normal-94da-20261007T2100Z: receipt SHA-256 `20df81485c5c860d3ed7ac1f32bdee8c838b7ce5ccf843e3311ad841c7395f95`; load min/median/max 5.61/8.45/10.64.
- series-main8f-20261007: receipt SHA-256 `ee845d2f37442786ee9147d9fa31dad7ce89d5d03a36a6461b5ac4c6edebfe22`; load min/median/max 4.24/9.29/11.58.
- series-recovery7b-20261007: receipt SHA-256 `dc97f9e73aba7a7ba133df6b5f9d3342577e75df5841f83cffdb65512994381a`; load min/median/max 3.50/4.34/5.17.
- series-wal8f-20261007: receipt SHA-256 `53ccfb5c97665ca74362ab5f9d5e3578ca11ce241b5d57cf379b09a919074251`; load min/median/max 3.28/3.72/4.06.
- focused-candidate-checkpoint-parent-20261007: receipt SHA-256 `4c6d538b921c0679052f015792cd21691731f52c015df73c3c975288029846d6`; load min/median/max 5.66/6.65/9.52.
- focused-candidate-checkpoint-release-20261007: receipt SHA-256 `3d95cb6f1a45285c893c9eebbec8b963dc8b659acaddfd0a0d1aa7c13a3665ff`; load min/median/max 5.15/5.68/13.56.
- focused-candidate-checkpoint-owned-parent-20261007: receipt SHA-256 `e6ba62e0aacc73d7944a92852d01752609553f70940e2475df28878d3e7ea512`; load min/median/max 3.47/4.66/5.93.
- focused-candidate-checkpoint-owned-release-20261007: receipt SHA-256 `2d429ed5c2209a09258316a296ece82d5ce1fb73cc37d98acf2d7476c84971d1`; load min/median/max 5.10/5.77/27.93.
