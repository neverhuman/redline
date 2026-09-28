# Perf case lists

## `medium-set.txt`: the historical 294-case medium cohort

`medium-set.txt` is restored byte for byte from
`git show 0f831a2bb^:bench/perf/cases/medium-set.txt`. Commit 0f831a2bb deleted it when it
assembled the monorepo. `medium-set.txt.sha256` holds its SHA-256 in `sha256sum` format
(`sha256sum -c medium-set.txt.sha256` from this directory).
`crates/bench/tests/medium_cohort_list.rs` fails if either file changes. Do not edit the list.
A different cohort needs a new file with its own name and digest.

**What it is.** These are the cases behind the README's historical "Version performance history"
rows (v4.0.8, v4.0.9 and v4.1.0: median/p95 latency ratio on 294 cases). Those rows were
measured by the retired `scripts/perf/medium.sh` lane. That lane used a Python replay driver,
2 workers pinned to CPUs 2-5, `REDLINEDB_DEFAULT_DURABILITY=normal` and `/dev/shm`, and the
v4.0.9/v4.1.0 builds were PGO-trained on this same cohort. The raw data was not retained, and
none of that lane can be rerun today. This file is kept so the cohort can be sliced out of a
new, fully recorded run, not so those rows can be reproduced.

**How it was selected.** `scripts/perf/build_case_lists.py` wrote it. The script was added in
bf7733e49 and committed with this list in 27132c4a3 (2026-05-25). Its inputs were the runner's
`list --suite sqlite_parity --format json` output, which had 1,127 cases then (see the
`corpus_size` header), and a `ranked.csv` of per-case medians from a run of that date. Which run
that was is not recorded. The script took, in order and without duplicates:

| Stratum (comment prefix) | Rule | Cases |
| --- | --- | ---: |
| `P0` | every P0 case | 130 |
| `P1-worst` | the 100 P1 cases with the worst RedlineDB/SQLite median ratio | 100 |
| `cat-spread` | 5 picks per major category: the worst, p75, median, p25 and best ratio | 45 |
| `abs-outlier` | the 30 cases with the highest RedlineDB median, less those already chosen | 19 |

It skipped cases 00093-00096, which the runner of that time skipped.

**Bias.** The cohort was chosen partly because its cases were slow or regressed in one run of
that date. Later measurements of such cases tend to look better through regression to the
mean alone, so this cohort favors later versions. It is not representative of the corpus.
Read it as a historical-comparability view only. A release bench bundle
(`scripts/perf/release-bench.sh`) reports the full-corpus common pass set as its primary
cohort and this list as a secondary slice.

**Today's corpus.** All 294 ids are in today's 2,445-case `sqlite_parity` corpus, within the
00001-01127 range of the corpus they were drawn from, and each id still has the name its
comment gives. Of the 294, 289 cases use the `memory` profile (`:memory:` databases) and 5 use
`tempfile`.
