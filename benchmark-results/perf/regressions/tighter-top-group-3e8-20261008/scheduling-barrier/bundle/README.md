# Scheduling-barrier interference diagnostic

This separate, predeclared crossover tests controller overlap after the retained one-core study and rejected affinity diagnostic. Engine and controller stay on CPU 23 in both conditions. Direct launch checks the running child; gated launch checks a waiting shell, then permits the exact binary to exec with the same PID, affinity, nice and I/O priority. The gate finishes all synchronous scheduling checks before the benchmark starts. The shell wrapper and barrier are explicit treatment differences, outside the engine's query clock. Immutable version-specific seed images, stable 927/current 3e8 binaries, full 20,000-row work, settings and result digests remain identical. The shared CPU is not exclusive. No peer affinity or scheduling changes.

Each case has 32 complete ABBA blocks per condition plus one retained warmup, with condition order alternating by block. Ratios are main/release; lower is better. Seeded 50,000-draw circular moving-block bootstrap over eight adjacent blocks produces exploratory 95% intervals. This does not qualify a release or establish equivalence. Elapsed-minus-process-CPU medians are timer differences, not direct kernel wait measurements; process CPU includes all child threads. Scheduling-check and gate-release timestamps and child schedstat snapshots remain in raw records. No timing or load sample is discarded.

| Case | Launch | Elapsed ratio |95% interval|Process CPU ratio|95% interval|Release elapsed−CPU ms|Main elapsed−CPU ms|
|---|---|---:|---:|---:|---:|---:|---:|
| top10 | direct | 1.223076 | 1.191463–1.242377 | 1.033429 | 1.023537–1.038529 | -0.052 | 12.616 |
| top10 | gated | 1.006664 | 1.002597–1.010071 | 1.007141 | 1.002301–1.008433 | -0.052 | -0.052 |
| join_group_by_city | direct | 1.214748 | 1.154585–1.244380 | 1.011184 | 1.003798–1.016756 | -0.052 | 11.858 |
| join_group_by_city | gated | 0.991952 | 0.987027–0.995293 | 0.992252 | 0.987264–0.994743 | -0.043 | -0.057 |
| point_pk_sql_text | direct | 1.045814 | 1.039569–1.159007 | 0.999012 | 0.994046–1.000604 | 0.071 | 13.146 |
| point_pk_sql_text | gated | 0.987372 | 0.982764–0.991749 | 0.989996 | 0.987061–0.992603 | 0.112 | 0.082 |

792 successful executions, 0 failures, 0 skips, including 24 warmups. Host xbabe3, engine CPU 23, nice 19 and idle I/O, load1 min/median/max 2.59/3.22/4.91. Receipt SHA-256 2a30008e1418576f0876e4b0fd3903fbdf10790b216ff4517f9d376ebbfe2bde. Both prior studies and their contradictory or inconclusive results remain preserved; this diagnostic does not rewrite their observations or attribute a unique engine source cause.
