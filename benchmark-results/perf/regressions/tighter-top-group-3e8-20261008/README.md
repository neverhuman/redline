# Tighter Top 10 and grouped-join investigation

Two complete studies and a separate scheduling-control crossover compare exact main 3e8b2d5b6f13a4440c10ad5c56bff73d3d967538 with v5.1.1 9277455d5ad008252053a81d18add39b8cdc8f7b. The completed datasets contain 5448 successful raw executions (4656 in the original studies and 792 in the crossover), zero failed engine executions and zero skips. A separate rejected diagnostic retains 175 partial records and an exit-1 metadata failure; it is not counted as passing evidence. All warmups and load/timing observations are preserved. Lower main/release query-time ratios are better.

| Study | Case | Main / release median | Decision interval | Confidence | Finding |
|---|---|---:|---:|---:|---|
| two-core | top10 | 1.009179 | 0.998003–1.054555 | 97.5% | inconclusive |
| two-core | join_group_by_city | 1.003248 | 0.989215–1.018074 | 97.5% | inconclusive |
| two-core | point_pk_sql_text | 1.003212 | 1.000305–1.008660 | 95% | inconclusive |
| one-core | top10 | 1.255862 | 1.062790–1.278604 | 98.75% | reproduced-slowdown |
| one-core | join_group_by_city | 1.260414 | 1.066784–1.283821 | 98.75% | reproduced-slowdown |
| one-core | point_pk_sql_text | 1.039218 | 1.021986–1.047905 | 95% | reproduced-slowdown |

The [first study](two-core/README.md) used two pinned CPUs and 64 ABBA blocks per case in each of two sessions. Both primary decisions stayed inconclusive. The [final confirmation](one-core/README.md) was declared afterward, uses one pinned logical CPU to prevent migration between pinned cores, and doubles the blocks. Its stricter primary interval uses a four-comparison Bonferroni correction as a conservative diagnostic; this does not establish fixed-sample familywise coverage for the adaptive investigation. Both sessions must independently agree before a direction is reproduced. No further sample extension was made. These are exploratory shared-host diagnostics; crossing 1.0 does not establish equivalence, and upper bounds apply only under the recorded conditions.

The original single-core launcher synchronously checked the running child on the same CPU. The [completed scheduling-barrier crossover](scheduling-barrier/bundle/README.md) holds engine/controller CPU, binaries, seed images and work constant, but completes scheduling checks before permitting the gated binary to exec. Direct launch reproduces the large elapsed slowdown; gated launch removes most of it and the main elapsed-minus-process-CPU gap. This supports controller overlap as the source of the large diagnostic slowdown. The waiting-shell treatment and shared-host conditions are explicit, so it does not establish a unique engine source cause.

| Case | Gated main / release median | Exploratory 95% interval |
|---|---:|---:|
| top10 | 1.006664 | 1.002597–1.010071 |
| join_group_by_city | 0.991952 | 0.987027–0.995293 |
| point_pk_sql_text | 0.987372 | 0.982764–0.991749 |

Neither primary gated interval lies wholly above the owner's 1.05 regression threshold. A small Top 10 difference remains; this single exploratory crossover neither establishes equivalence nor qualifies a future release. The [rejected affinity precursor](rejected-affinity/bundle/receipt.json) failed when a scheduling probe inspected an already-exited child; its partial records have no computed timing interval. Three [historical candidate builds](historical-builds/) are preserved for attribution, but no completed historical timing bisect is claimed.

The original release throughput tables remain bound to their publishable bundles. No absolute README figure is replaced by these query-timer diagnostics. The future launcher uses the verified pre-exec barrier; CI regression tests exercise waiting, same-PID execution, failed checks, EOF, literal arguments and child failure without running timings in CI.

[Input manifest](investigation.json) pins every session/diagnostic receipt, off-root archive hash and supplemental source/build/verification log. Raw plans, records, scheduling and core/SMT/load telemetry are committed within each dataset. Every archive payload inventory was independently hash-verified before import. The cache target symlink was removed after the first complete study; builds use a per-command external Cargo target directory. Default and explicit-target CI checks retain every original lane.
