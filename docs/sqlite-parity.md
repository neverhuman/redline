# SQLite Parity Traceability Ledger

This ledger records RedlineDB's current SQLite-facing compatibility status.
The reference oracle is the SQLite library bundled with `rusqlite` in the test
harness. RedlineDB-side parity tests are local regression checks only; they do
not produce SQLite parity coverage, benchmark, report, sentinel, or proof
evidence artifacts.

## Official reference shell

`scripts/sqlite/build-reference.sh` builds the shell the official lane compares
against. A `sqlite3` already on `PATH`, or a previously built tree under
`target/sqlite-reference/` whose compile flags differ from that script, is a
diagnostic.

The 3.53.1 autoconf amalgamation ships a parser generated without
`SQLITE_UDL_CAPABLE_PARSER`. `UPDATE`/`DELETE` `… ORDER BY … LIMIT` is therefore
a parse error (`near "ORDER": syntax error`, exit 1) even when the `cc` line
passes `-DSQLITE_ENABLE_UPDATE_DELETE_LIMIT`. That flag is present in
`scripts/sqlite/build-reference.sh` and in the compiler log, and cases 00219 and
00220 still expect rejection. The same script does not pass `-DSQLITE_SOUNDEX`,
so `soundex()` is a missing function. `-DSQLITE_ENABLE_SESSION` is on, and
`.session` with no arguments exits 0 and prints the session help.

The official corpus and gate live in `subrepos/redline-testing` in this checkout.
That runner is the sole official producer of parity evidence; engine-local
regression tests do not produce public parity reports. CI builds the runner and
engine from the same parent commit, stages corpus/metadata/schema/template files,
and verifies binary and evidence hashes before accepting the processed bundle.
Two provenance files sit next to the committed report, and they are never
merged:

- `run-provenance.json` is the run's own `target/redline-testing/provenance.json`
  (schema `redline-testing-run-provenance-v2`), staged unchanged by
  `just sqlite-parity-report-update` after its SHA-256 matches
  `suite_summaries.sqlite_parity.provenance_sha256` in the processed evidence.
  It names the measured target, SQLite oracle and runner (path, SHA-256,
  version), the source commit and tree, `source_inputs_sha256` (the
  `.github/parity-report-inputs.sha256` recipe), `source_dirty` from
  `git status --porcelain` over those inputs (untracked files included), the
  corpus and assertion-policy hashes, the oracle build stamp from
  `scripts/sqlite/build-reference.sh`, and the suite's elapsed time.
- `report-provenance.json` (schema `redline-testing-report-provenance-v1`)
  records the rendering: the parent run provenance hash, the run and processed
  evidence hashes, the raw hash, the renderer, the arguments and the hashes of
  every output (for the README, only the generated blocks). It holds no Git
  state.

`redline-testing report` takes every measured identity from the processed
evidence and the run provenance, checks them against each other and against
every raw sample, and refuses unknown identities, a run provenance the
evidence does not bind, and a dirty measured source. It never probes `PATH`,
`REDLINE_TESTING_*_BIN` or Git, so `just sqlite-parity-report-check` needs no
rewriting of committed files. The committed 2026-09-24 run predates run
provenance: it renders only with `--historical-run`, which the renderer refuses
for evidence that names a run provenance, and its report is marked historical.
Older committed reports retain their original dates; migration does not rewrite
historical measurements.

Status values are deliberately narrow:

| Status | Meaning |
|---|---|
| `pass` | Covered by an executable test and expected to match the bundled SQLite reference for the stated row. |
| `partial` | Covered subset is tested against the bundled SQLite reference, but the row still has documented SQLite behavior outside that subset. |
| `fail` | SQLite supports the row, but RedlineDB currently rejects it or implements only a documented subset. |
| `not-started` | No production implementation exists yet, or the current implementation intentionally uses RedlineDB-native behavior instead of SQLite behavior. |
| `rejects-by-design` | SQLite accepts or no-ops the row, but RedlineDB intentionally rejects it with an explicit `UnsupportedSql` boundary instead of counting it as parity. |

## Corpus expectations

Each case's declared expectations (`expected_exit`, `expected_stdout` and the
`expected_*_contains` fragments) describe the pinned shell. `xtask generate`
and `xtask ship-gate` in `subrepos/redline-testing` default to it and refuse
any shell whose build stamp is missing or differs from the one
`scripts/sqlite/build-reference.sh` writes now (its archive SHA3-256 and
compile flags); `ship-gate` checks the pinned manifest as
well as the shards. Before 2026-09-27 both ran the `sqlite3` on `PATH`
(Ubuntu's 3.45.1), so 141 cases declared 3.45.1 behaviour: 127 shard cases and
14 manifest cases. The generated `gen_*` shards were regenerated. Hand-written
shard cases took the pinned shell's stdout where 3.53.1 prints differently:
column, box, table, line, html, tabs and quote modes, round-trip precision
for reals (up to 17 digits), `.tables` column width, `sqlite_schema` in
`.schema`, mode flags that reset an earlier `-header`, `-separator`, `-nullvalue` or `-newline`
(10201, 10203-10205), and `PRAGMA secure_delete` = 0 (10254).

The pinned manifest (`generated_manifest.json`, cases 1-1127) is otherwise
frozen. These manifest cases were migrated by hand to the pinned shell. The
runner checks each case's declared contract on the reference before it compares
the engines, and every case that expects a non-zero exit must declare a stderr
or combined fragment. That is why 00142 changed.

| Case | Name | Was (3.45.1 or unverified) | Now (3.53.1) |
|---|---|---|---|
| 00142 | `DOT_EXIT_CODE` | `.exit 7` alone, which prints nothing | prints `before-exit`, exits 7, and never runs the statement after `.exit` |
| 00110, 00176 | `DOT_MODE_LINE_COLUMN_TABLE_BOX_MARKDOWN`, `OPT_LINE_MODE` | line mode fragment `a = 1` | `a: 1` (and `b: x`) |
| 00163 | `DOT_FILECTRL_CATALOG` | exit 0 | exit 1, prints `Available file-controls:` |
| 00164 | `DOT_IMPOSTER_CATALOG` | exit 0 | exit 1, `Usage: .imposter INDEX IMPOSTER` |
| 00167 | `DOT_UNMODULE_CATALOG` | exit 0 | exit 1, unknown command `unmodule`: the pinned build is not `SQLITE_DEBUG`, the only build with `.unmodule` (declared shared rejection) |
| 00168 | `DOT_CHECK_CATALOG` | exit 0 | exit 1, `no .testcase is active` |
| 00171 | `OPT_HELP` | exit 1 | exit 0; help text on stderr |
| 00178 | `OPT_HTML_MODE` | fragment `<TD>1</TD>` | `<TD>1` then `<TD>x`, no closing tags |
| 00194 | `OPT_IFEXISTS_NEGATIVE_TEMPFILE` | `unknown option: -ifexists` | `-ifexists` refuses the missing file |
| 00199 | `OPT_PAGECACHE` | stdout `1` | a `Page cache size increased ...` line, then `1` |
| 00219, 00220 | `UPDATE_LIMIT_OPTIONAL`, `DELETE_LIMIT_OPTIONAL` | exit 0 with updated rows | exit 1, `near "ORDER": syntax error` (declared shared rejection) |
| 00222 | `OPT_ESCAPE_SYMBOL` | fragment `\n` (RedlineDB's output) for `SELECT char(10)` | `SELECT char(1)\|\|'x'` prints `␁x` (U+2401); a newline is unescaped in every mode, so the old statement could not tell whether `-escape` did anything |
| 00226 | `OPT_NOFOLLOW_CATALOG` | exit 0 | exit 1, `-nofollow` refuses the missing file |

The target is held to the same declared contract: a RedlineDB run that exits
with a different code, or lacks a declared fragment, fails as
`target_semantic_failure` before the shells are compared (case 10547 declares
`UNIQUE constraint failed: t.x`, and a `no such table: t` no longer passes). The
comparison itself is byte for byte: stdout and stderr are captured as raw bytes
and nothing is trimmed, folded or decoded, so a trailing space, a missing empty
row, a CR inside a field and a U+FFFD printed for a raw byte are all
`differential_mismatch`. A case can opt into `comparison_mode: cli_text_lf`
(CRLF read as LF, for a case about platform line framing) or drop whole lines
with `ignore_line_prefixes` (case 00208 drops its `trace.xRandomness(` seed
lines); declared fragments and `expected_stdout` are still matched on
normalized text. Every case with `compare_stdout: false` states why in
`stdout_uncompared_reason`; the 2026-09-27 audit turned stdout comparison on for
00115, 00117, 00118, 00121, 00124, 00128, 00129, 00133, 00140, 00141, 00152,
00158, 00159, 00222 and 10182, whose output is the same bytes on every run.

Failures are published, not hidden. `metadata/sqlite_parity/known-failures.json`
lists every `sqlite_parity` and `memory` case the current target fails, with
the verdict it fails with, why, and the phase that owns the fix. The official
run (`--sqlite-known-failures`) and `evidence_processor` both require each
suite's failed cases to be exactly that list: an unlisted failure fails the
run, and so does a listed case that passes, which leaves the file in the commit
whose raw result shows the pass. A feature row below whose feature has a
listed failure (for example 10560 for savepoints, 10253 and 10568 for
`PRAGMA query_only`, 10584 for foreign keys) names that case and is not
`pass`.

Skips are listed the same way; there is no skip budget.
`subrepos/redline-testing/corpus/sqlite_parity/scope-policy.json` is compiled
into the runner and lists every case a `sqlite_parity`, `memory` or
`rql_phase1` run may skip, with the gap it covers, why, who closes it, and an
expiry date. Today it lists only the 202 `rql_phase1` cases whose SQL the RQL
phase-1 rewriter cannot express. A gap it does not list fails its case
(`target_unsupported`), and a reference shell without a capability a case
declares always fails it (`reference_capability_missing`). An unknown
capability token, an unknown case status or a capability probe that cannot run
is an error. The official lane runs `redline-testing run --official`, which
refuses `REDLINE_TESTING_PINNED_ONLY`, `--case-id` and an expired exception,
and `evidence_processor` accepts only official evidence whose skipped case ids,
recomputed from the raw records, equal the policy's list.

Each case has exactly one verdict. `redline-testing check-sqlite`, which
`scripts/process-redline-testing-evidence.sh` runs before `evidence_processor`,
reduces each SQLite-shell suite to failed (any failed record, a failed warmup
included), skipped (a policy skip) or passed, and fails unless every executed
case has exactly the run's warmups and repetitions once each and the cases are
exactly the suite's corpus. `evidence_processor` requires the resulting
`sqlite-qualification.json` from the verified runner and this run, and checks
its disjoint id lists against the raw records and the published counts; the
report applies the same reduction.

Cases the pinned build cannot express are declared in
`subrepos/redline-testing/metadata/sqlite_parity/declared-deviations.json`
and listed in the report: the `soundex()` cases 11437-11439 and 00219/00220
record the shared rejection, and 10546, written to show that a default build
lacks `median()`, now checks `median()` because the pinned build enables
`SQLITE_ENABLE_PERCENTILE`.

## Feature rows

The tables below are generated from `docs/sqlite-feature-matrix.json` by
`scripts/parity/render-sqlite-feature-matrix.sh`; edit the JSON and rerun the
script, never the tables. `scripts/parity/lint-sqlite-parity-ledger.sh` (CI
preflight) fails when the tables differ from the JSON, when a listed test
function or evidence path does not exist, when a corpus case id is not in the
official catalog, or when a `pass` row lists an open subfeature.

The Proof column says what kind of evidence the listed tests are:

| Proof | Meaning |
|---|---|
| `semantic_match` | The tests compare RedlineDB with SQLite: the bundled `rusqlite` oracle, the official corpus, or values taken from SQLite. |
| `readback_only` | The tests show RedlineDB accepts the statement and reads back its own value; no SQLite value is claimed. |
| `intentional_reject` | The tests assert the explicit rejection. |
| `known_divergence` | A sentinel test asserts that RedlineDB differs from SQLite. |
| `none` | No test. |

<!-- sqlite-feature-matrix:begin -->

## SQL Surface

| Feature row | Status | Proof | Tests | Owner | Notes |
|---|---|---|---|---|---|
| Basic `SELECT` projection/filter/order | pass | semantic_match | `crates/sql/tests/differential_lab.rs`: `diff_scalar_string_matrix`, `diff_null_semantics_matrix`; `crates/sql/tests/smoke_select.rs`: `create_insert_select_round_trip`, `select_distinct_deduplicates_rows` | sql-parser-planner-executor | Covered for scalar values and simple predicates. |
| `INSERT`, `UPDATE`, `DELETE` basics | pass | semantic_match | `crates/sql/tests/smoke_dml.rs`: `insert_select_populates_target_rows`, `update_to_indexed_column_moves_index_entry`, `delete_removes_index_entry`; `crates/bench/compat/**` | sql-parser-planner-executor | Cross-engine SQLLogicTest corpus covers representative DML. Corpus cases: 10547–10548. |
| `CREATE TABLE`, `DROP TABLE` basics | partial | semantic_match | `crates/sql/tests/parity_coverage.rs`: `ctas_table_info_metadata_matches_sqlite`; `crates/bench/compat/**` | sql-parser-planner-executor | The official sqlite_parity corpus covers basic create/drop. Corpus cases: 10579–10582. Open: SQLite metadata compatibility is not complete. |
| `CREATE TABLE AS SELECT` | pass | semantic_match | `crates/sql/tests/parity_coverage.rs`: `ctas_basic_data_and_row_order_match_sqlite`, `ctas_table_info_metadata_matches_sqlite`, `ctas_if_not_exists_short_circuits_before_source_bind`, `ctas_rolls_back_on_source_runtime_error` | sql-parser-planner-executor | CTAS names, declared types, row order, `IF NOT EXISTS` short-circuiting, and rollback on source-query failure are covered against `rusqlite`. |
| `CREATE INDEX`, `DROP INDEX` basics | pass | semantic_match | `crates/sql/tests/parity_coverage.rs`: `create_and_drop_index`, `drop_index_if_exists`, `reindex_executes_after_index_creation` | sql-parser-planner-executor | Basic B-tree-backed indexes only. |
| `ALTER TABLE RENAME TABLE/COLUMN` | partial | semantic_match | `crates/sql/tests/parity_coverage.rs`: `alter_table_rename_to`, `alter_table_rename_column` | sql-parser-planner-executor | The official sqlite_parity corpus covers rename of tables and columns. Corpus cases: 10413, 10418, 10425. Open: ADD COLUMN and DROP COLUMN variants are outside the covered subset; a CHECK on an added column is not visible to the next INSERT (case 10413). |
| `RETURNING` expressions | pass | semantic_match | `crates/sql/tests/parity_coverage.rs`: `returning_with_arithmetic_expression`, `returning_with_function_call`, `update_returning_with_expression` | sql-parser-planner-executor | Insert/update expressions covered. |
| UPSERT `ON CONFLICT DO UPDATE` representative path | partial | semantic_match | `crates/sql/tests/phase10_sqlc_conflict_matrix.rs`: `upsert_do_update_updates_correct_row`, `upsert_do_update_with_where_predicate_applies_when_true`, `upsert_do_nothing_leaves_table_unchanged`; `crates/bench/compat/orm/migration.sqlt` | sql-parser-planner-executor | The official sqlite_parity corpus covers the representative `ON CONFLICT DO UPDATE` path. Open: the full SQLite conflict matrix is outside the covered subset. |
| Savepoints | partial | semantic_match | `crates/sql/tests/parity_coverage.rs`: `nested_savepoint_basic`, `nested_savepoint_release`; `crates/sql/tests/phase10_sqlb.rs`: `rollback_to_rewinds_post_savepoint_inserts`, `nested_savepoints_independent_rewinds` | sql-parser-planner-executor | Nested savepoint behavior has focused coverage. Corpus cases: 10560. Open: ROLLBACK TO re-executes the statements journaled before the savepoint instead of undoing exactly, and refuses (failing the transaction) when one of them is not replay-safe. |
| Joins and left joins | pass | semantic_match | `crates/sql/tests/differential_lab.rs`: `diff_outer_and_cross_join_matrix`, `diff_natural_using_join_output_shape`; `crates/sql/tests/parity_coverage.rs`: `inner_join_chain`; `crates/sql/tests/smoke_select.rs`: `left_join_null_extends_missing_rows`, `natural_using_join_merged_star_and_left_order`; `crates/bench/compat/orm/queries.sqlt` | sql-parser-planner-executor | Explicit join forms are covered. NATURAL JOIN and USING merged-column output semantics are implemented (W9-T9, `72b4107`); corpus cases 10445/10451/10466 pass. Cross joins, self-joins, and multi-way joins have coverage via `differential_lab`. Corpus cases: 10445, 10451, 10466. |
| Row-value `IN` subqueries | pass | semantic_match | `crates/sql/tests/differential_lab/query_shapes.rs`: `diff_subquery_matrix`; `crates/sql/tests/parity_coverage.rs`: `row_value_in_subquery_matches` | sql-parser-planner-executor | Representative row-value `IN`/`NOT IN` covered. |
| Correlated subqueries | pass | semantic_match | `crates/sql/tests/differential_lab.rs`: `diff_correlated_subquery_outer_pk_is_not_inner_rowid_alias`; `crates/sql/tests/differential_lab/query_shapes.rs`: `diff_subquery_matrix` | sql-parser-planner-executor | A7: thread-local outer-row stack resolves qualified outer-scope references; correlated `EXISTS` / `NOT EXISTS` and correlated scalar subqueries in projection covered against the rusqlite oracle. Corpus cases: 10502, 10585. |
| CTEs (`WITH`, recursive and non-recursive) | partial | semantic_match | `crates/sql/tests/launch_cte_scope.rs`: `cte_name_does_not_shadow_table_in_later_statement`, `nested_with_in_derived_table_keeps_outer_rows`; `crates/sql/tests/launch_view_freshness.rs`: `cte_parameter_bound_after_prepare`, `an_unused_cte_naming_a_missing_table_is_an_error`; `crates/sql/tests/parity_cte.rs`: `non_recursive_cte_basic`, `recursive_cte_fibonacci_first_eight`, `recursive_cte_cycle_dedups_with_union`, `non_recursive_cte_materialized_hint_is_accepted` | sql-parser-planner-executor | A3: non-recursive + recursive (UNION / UNION ALL) materialized when the statement is bound, into rows the statement owns (Q5-09); a statement is bound again at each execution with its current parameters (Q5-08). Accepts `AS MATERIALIZED` / `AS NOT MATERIALIZED` as no-op hints and supports JOIN against CTE via synthetic TableDef. Iteration cap 10_000. Open: declared deviation: every CTE is materialized, used or not, so an unused CTE that names a missing table is an error where SQLite ignores the CTE. |
| Compound `SELECT` (`UNION`, `INTERSECT`, `EXCEPT`) | pass | semantic_match | `crates/sql/tests/parity_compound_select.rs`: `union_all_keeps_duplicates`, `intersect_returns_common_rows`, `except_returns_left_only_rows`, `mixed_set_operations_follow_sqlite_left_to_right`; `crates/sql/tests/parity_order_by_ordinal.rs`: `compound_union_all_order_by_position` | sql-parser-planner-executor | A2: UNION / UNION ALL / INTERSECT / EXCEPT all wired through the parser and exec layers; column-arity and type-class mismatches return diagnostic errors that match SQLite's error class. Parameterized compound branches and trailing wrapper `ORDER BY` / `LIMIT` share stable parameter slots, and top-level integer literals in trailing `ORDER BY` resolve as 1-based output-column references (single-branch and compound), matching SQLite. Corpus cases: 10578. |
| Window functions and frames | pass | semantic_match | `crates/sql/tests/parity_window.rs`: `row_number_over_order_by`, `rank_dense_rank_with_ties`, `running_sum_rows_frame`, `frame_exclude_group_sum` | sql-parser-planner-executor | A4: ROW_NUMBER / RANK / DENSE_RANK / NTILE / LAG / LEAD / FIRST_VALUE / LAST_VALUE / NTH_VALUE / PERCENT_RANK / CUME_DIST + aggregate-OVER (SUM/COUNT/AVG/MIN/MAX/TOTAL) with ROWS / RANGE / GROUPS frames. |
| Views | pass | semantic_match | `crates/sql/tests/launch_view_freshness.rs`: `held_view_statement_prepared_before_insert_sees_row`, `create_view_with_parameter_rejected`; `crates/sql/tests/parity_view.rs`: `create_select_drop_view_basic`, `repeated_prepare_of_view_observes_base_table_changes`, `insert_into_view_is_rejected`, `view_survives_reopen` | sql-parser-planner-executor | The official sqlite_parity corpus covers `CREATE [TEMP] VIEW [IF NOT EXISTS]` and `DROP VIEW [IF EXISTS]`. Views persist in the catalog (format_version 4) and expand at FROM-binding time; a statement that reads a view, a CTE or a derived table binds again at each execution, so a held or reset statement reads current rows (Q5-08). A view body is bound in its own scope, and parameters in `CREATE VIEW` are refused as in SQLite. DML on a view returns a SQLite-style "cannot modify view" error. Corpus cases: 10572–10573. |
| Triggers | partial | semantic_match | `crates/sql/tests/launch_trigger_chains.rs`: `distinct_trigger_chain_runs_with_recursion_off`, `self_trigger_is_skipped_when_off_and_nests_when_on`, `instead_of_self_insert_does_not_recurse`, `trigger_depth_cap_boundary`, `failing_body_leaves_no_partial_effects`; `crates/sql/tests/parity_trigger.rs`: `after_insert_writes_audit_row`, `after_update_of_column_filters_firing`, `recursive_trigger_terminates_at_cap`, `recursive_triggers_off_still_fires_a_distinct_nested_trigger`, `trigger_survives_reopen` | sql-parser-planner-executor | The official sqlite_parity corpus covers `CREATE TRIGGER {BEFORE\|AFTER} {INSERT\|UPDATE [OF col,...]\|DELETE}` plus `DROP TRIGGER`, including `OLD`/`NEW` and `UPDATE OF`. `recursive_triggers` follows SQLite: when off, only a trigger that is already running is not fired again, so chains of distinct triggers run to any depth; when on, triggers nest up to the cap. A failed trigger fails its statement: outside an explicit transaction the statement changes nothing, and inside one the transaction must be rolled back. Corpus cases: 10561, 10570. Open: INSTEAD OF UPDATE and INSTEAD OF DELETE on views (INSTEAD OF INSERT fires); trigger programs nest at most 8 deep, where SQLite allows 1000; the ninth fails its statement with "too many levels of trigger recursion", which the C API reports as SQLITE_MISUSE rather than SQLITE_ERROR; a CASE ... END expression inside a trigger body fails to parse; a dropped trigger's name cannot be used again: CREATE TRIGGER fails with "object already exists"; RAISE outside a trigger raises instead of being rejected (case 10570). |
| Generated columns | pass | semantic_match | `crates/sql/tests/parity_generated_col.rs`: `generated_stored_basic_insert_select`, `generated_virtual_basic_insert_select`, `generated_stored_recomputes_on_update`, `generated_stored_write_block_explicit_insert` | sql-parser-planner-executor | A6 SQL-D: `STORED` and `VIRTUAL` generated columns parse, persist (catalog format_version 7), recompute on INSERT/UPDATE (STORED), and evaluate on read (VIRTUAL). Writes targeting generated columns are rejected with a SQLite-class error. |
| Partial indexes | pass | semantic_match | `crates/sql/tests/parity_partial_index.rs`: `partial_index_indexes_only_matching_rows`, `partial_index_update_moves_in_and_out`, `partial_index_planner_only_uses_matching_predicate` | sql-parser-planner-executor | A6 SQL-D: `CREATE INDEX ... WHERE <predicate>` persists the predicate as verbatim SQL on `IndexDef.predicate_sql`; DML re-parses and evaluates the predicate per row (`crates/sql/src/exec/index_predicate.rs`) so partial indexes track in/out membership precisely, and reads only use the index when the query predicate implies the index predicate (`crates/sql/src/exec/index_partial.rs`). |
| Expression indexes | pass | semantic_match | `crates/sql/tests/parity_expr_index.rs`: `expression_index_lower_lookup`, `expression_index_update_recomputes_key`, `expression_index_delete_removes_key` | sql-parser-planner-executor | A6 SQL-D: `CREATE INDEX ... ON t(expr(col))` stashes the expression SQL on `IndexKeySource::Expression`; DML re-evaluates the expression to compute the index key, and UPDATE re-emits when any referenced column changes. |
| Foreign keys | pass | semantic_match | `crates/sql/tests/parity_fk_enforce.rs`: `insert_into_child_with_missing_parent_errors`, `delete_parent_cascade_removes_children`, `deferred_fk_unresolved_at_commit_errors`; `crates/sql/tests/phase10_sqld_fk.rs`: `foreign_key_referential_actions_parse` | sql-parser-planner-executor | A6-fk: PRAGMA-gated enforcement on INSERT/UPDATE/DELETE; ON DELETE/UPDATE {NO ACTION, RESTRICT, CASCADE, SET NULL, SET DEFAULT}; DEFERRABLE INITIALLY DEFERRED checked at COMMIT. Cascade depth bounded. Corpus cases: 10584. |
| ATTACH / DETACH | partial | semantic_match | `crates/sql/tests/parity_attach.rs`: `attach_via_rusqlite_oracle_smoke`, `select_join_across_attached_alias_returns_rows`, `cross_db_insert_select_copies_main_rows_to_attached_database` | sql-parser-planner-executor | Corpus cases 10372–10389 (category SQL_ATTACH) pass: file and in-memory ATTACH, DETACH, qualified table references, same-named tables in separate schemas, cross-database JOIN and `INSERT ... SELECT`, a transaction spanning the attached databases, `.databases`, `PRAGMA database_list` and per-database `user_version`, alias-qualified `UPDATE`/`DELETE`, and the error cases. Corpus cases: 10372–10389. Open: cross-database INSERT/UPDATE/DELETE without a SELECT subquery; cross-database transaction semantics. |

## Expressions And Functions

| Feature row | Status | Proof | Tests | Owner | Notes |
|---|---|---|---|---|---|
| NULL comparison and `IN`/`NOT IN` edge cases | pass | semantic_match | `crates/sql/tests/parity_coverage.rs`: `null_in_empty_list`, `value_in_list_with_null`, `value_not_in_list_with_null`, `null_comparison_is_null` | sql-parser-planner-executor | SQLite three-valued logic has focused coverage. |
| Core string scalars (`substr`, `trim`, `instr`, `replace`, case conversion, length) | pass | semantic_match | `crates/sql/tests/differential_lab.rs`: `diff_scalar_string_matrix`; `crates/sql/tests/parity_scalar_funcs.rs`: `substr_negative_start`, `trim_custom_chars`, `instr_empty_needle_returns_one`, `replace_all_occurrences` | sql-parser-planner-executor | Representative SQLite behavior covered. |
| Formatting and conditional scalars (`printf`, `format`, `iif`, `sign`) | pass | semantic_match | `crates/sql/tests/parity_scalar_funcs.rs`: `printf_integer_placeholder`, `format_is_alias_for_printf`, `iif_null_condition_returns_false_branch`, `sign_negative` | sql-parser-planner-executor | Does not imply exhaustive SQLite format coverage. |
| Blob/character helpers (`zeroblob`, `randomblob`, `char`, `unicode`) | pass | semantic_match | `crates/sql/tests/parity_scalar_funcs.rs`: `zeroblob_correct_length`, `randomblob_correct_length`, `char_basic_ascii`, `unicode_multibyte_literal_returns_first_codepoint` | sql-parser-planner-executor | Randomness is shape-tested, not value-matched. |
| Aggregate functions (`count`, `sum`, `total`, `min`, `max`) | pass | semantic_match | `crates/sql/tests/differential_lab.rs`: `diff_aggregate_matrix`; `crates/sql/tests/parity_agg_funcs.rs`: `total_vs_sum_null_difference`, `aggregate_distinct_and_filter_match_sqlite`, `group_concat_skips_nulls` | sql-parser-planner-executor | Representative grouped and NULL behavior covered. Corpus cases: 10586. |
| JSON aggregate functions | pass | semantic_match | `crates/sql/tests/parity_agg_funcs.rs`: `json_group_array_basic`, `json_group_array_includes_nulls`, `json_group_object_basic`, `json_group_object_skips_null_keys` | phase10-json1-surface | JSON aggregate rows covered. |
| JSON scalar functions | pass | semantic_match | `crates/sql/tests/parity_json1.rs`: `parity_json_extract_object_key`, `parity_json_set_overwrites_existing_key`, `parity_json_patch_merges_objects`, `parity_arrow2_extracts_scalar_text`; `crates/sql/tests/phase10_j1_compat.rs`: `phase10_j1_json_extract_multi_path_returns_array` | phase10-json1-surface | `parity_json1.rs` runs a rusqlite-oracle differential pass over `json()` / `json_array[_length]` / `json_object` / `json_extract` / `json_type` / `json_valid` / `json_quote` / `json_set` / `json_insert` / `json_replace` / `json_remove` / `json_patch` and the `->` / `->>` arrow operators. |
| Date/time functions | pass | semantic_match | `crates/sql/tests/phase10_sqld_datetime.rs`: `julianday_known_value_matches_sqlite`, `modifier_start_of_day_matches_sqlite`, `modifier_minus_one_month_matches_sqlite`, `strftime_full_format` | phase10-datetime | A8: `date()` / `time()` / `datetime()` / `julianday()` / `unixepoch()` / `strftime()` accept the SQLite time-string formats and modifiers (`'now'`, `'+/-N {days,hours,...}'`, `'start of {day,month,year}'`, `'weekday N'`, `'utc'`, `'localtime'`) through the shared modifier pipeline in `crates/sql/src/datetime/modifiers.rs`; differential parity against the rusqlite oracle is wired and green. |
| Collations | partial | semantic_match | `crates/sql/tests/parity_order_by_ordinal.rs`: `order_by_collate_nocase_matches_sqlite`, `order_by_collate_rtrim_matches_sqlite`; `crates/sql/tests/phase10_sqld_collation_unique_index.rs`: `nocase_unique_index_blocks_duplicate`; `crates/sql/tests/sqlite_full_parity.rs`: `known_full_sqlite_parity_gaps_are_explicit_failures` | phase10-collations | Explicit `ORDER BY ... COLLATE NOCASE/RTRIM/BINARY` and corpus case 10340 (`CREATE UNIQUE INDEX ... COLLATE NOCASE`) pass. Corpus cases: 10340. Open: a column declared COLLATE NOCASE does not order by that collation (sentinel known_full_sqlite_parity_gaps_are_explicit_failures); LIKE with a NOCASE index optimization. |
| User-defined SQL functions/collations | partial | semantic_match | `crates/ffi/tests/collation_register.rs`: `reverse_nocase_orders_descending`; `crates/ffi/tests/udf_register.rs`: `times_two_scalar_udf_invoked_from_select`, `aggregate_udf_sum_squares_invoked_per_group`, `window_callbacks_rejected_and_destroyed` | c-abi | Scalar and aggregate functions (`sqlite3_create_function{,_v2,16}`, and `sqlite3_create_window_function` without `xValue`/`xInverse`) and collations (`sqlite3_create_collation{,_v2}`) are called from prepared statements. A registration belongs to its connection: replacing it, deleting it (every callback NULL) or closing the connection releases `user_data` through its destructor once. Partial because `SQLITE_DIRECTONLY` and unknown flags, and window callbacks, are refused with `SQLITE_ERROR`, and the `sqlite3_collation_needed` callback is stored per connection but never called. See `docs/security-capabilities.md`. Open: window functions with xValue/xInverse are refused; the sqlite3_collation_needed callback is stored but never called. |

## PRAGMAs

| Feature row | Status | Proof | Tests | Owner | Notes |
|---|---|---|---|---|---|
| `PRAGMA integrity_check` / `quick_check` | pass | semantic_match | `crates/sql/tests/parity_coverage.rs`: `pragma_integrity_check_ok`, `pragma_quick_check`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces` | sql-parser-planner-executor | Current checks are RedlineDB-native integrity summaries. |
| `PRAGMA auto_vacuum` | partial | known_divergence | `crates/sql/tests/parity_coverage.rs`: `pragma_auto_vacuum`; `crates/sql/tests/parity_pragma_tv.rs`: `pragma_auto_vacuum_is_accepted_as_recall_only`; `crates/sql/tests/sqlite_full_parity.rs`: `known_gap_pragmas_are_rejected_or_diverge` | sql-parser-planner-executor | Accepted. `PRAGMA auto_vacuum = N` is stored and read back. On an empty database the readback equals SQLite's. Open: RedlineDB never vacuums; the value is stored and echoed; SQLite ignores a change once the database has a table, so the readback diverges there. |
| `PRAGMA wal_checkpoint(MODE)` | fail | known_divergence | `crates/sql/tests/parity_coverage.rs`: `pragma_wal_checkpoint_full`; `crates/sql/tests/parity_pragma_tv.rs`: `pragma_wal_checkpoint_is_accepted`; `crates/sql/tests/sqlite_full_parity.rs`: `known_gap_pragmas_are_rejected_or_diverge` | storage-and-catalog | Accepted for every mode as a no-op. RedlineDB has its own write-ahead log, not SQLite WAL frames, so there is nothing to report in SQLite's terms. Open: answers (0, 0, 0) where SQLite reports (busy, log frames, checkpointed frames), or -1 for the frame counts outside WAL mode. |
| `PRAGMA journal_mode` (memory/off/delete/wal) | pass | semantic_match | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_journal_mode_round_trips_supported_values`, `pragma_journal_mode_accepts_wal_and_reads_back_wal`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces` | sql-parser-planner-executor | Round-trip on the supported subset plus RedlineDB's WAL-style `wal` mode; `truncate` and `persist` are out of scope because SQLite page-format WAL semantics are not implemented. |
| `PRAGMA synchronous` | pass | semantic_match | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_synchronous_round_trips_supported_values`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces` | sql-parser-planner-executor | Accepts `OFF`/`NORMAL`/`FULL`/`EXTRA` plus integer aliases; stored for read-back. |
| `PRAGMA temp_store` | pass | semantic_match | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_temp_store_round_trips`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces` | sql-parser-planner-executor | Accepts `DEFAULT`/`FILE`/`MEMORY` plus integer aliases. |
| `PRAGMA cache_size` | pass | semantic_match | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_cache_size_round_trips_signed_values`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces` | sql-parser-planner-executor | Signed integer round-trip (negative = KiB, positive = pages). |
| `PRAGMA query_only` | pass | semantic_match | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_query_only_round_trips_and_blocks_writes`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces` | sql-parser-planner-executor | When ON, blocks every write-side statement with SQLite's `attempt to write a readonly database` (SQLITE_READONLY). Corpus cases: 10253, 10568. |
| Unknown PRAGMA names | rejects-by-design | intentional_reject | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_unknown_name_is_rejected`; `crates/sql/tests/sqlite_full_parity.rs`: `explicit_reject_pragmas_are_rejected_by_redline` | sql-parser-planner-executor | Unsupported PRAGMAs return `UnsupportedSql` naming the PRAGMA. `EXPLICIT_REJECT_REFERENCE_PRAGMAS` in `crates/sql/tests/sqlite_full_parity.rs` lists the ones the bundled SQLite supports. |
| `PRAGMA foreign_keys` | pass | semantic_match | `crates/sql/tests/parity_fk_enforce.rs`: `pragma_foreign_keys_off_skips_enforcement`, `fk_disabled_via_explicit_pragma_skips_checks`; `crates/sql/tests/phase10_sqld_fk.rs`: `pragma_foreign_keys_toggle_round_trips` | sql-parser-planner-executor | A6: per-connection toggle now gates the FK enforcement layer end-to-end; OFF skips every check (matches SQLite's bundled default). |
| Table-valued PRAGMAs | partial | semantic_match | `crates/sql/tests/parity_pragma_tv.rs`: `pragma_table_info_tv_form_matches_sqlite`, `pragma_index_list_tv_form_matches_sqlite`, `pragma_index_info_tv_form_matches_sqlite`, `pragma_foreign_key_list_tv_form_shape_matches_sqlite`; `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragma_rows_match_for_supported_surfaces`, `known_gap_pragmas_are_rejected_or_diverge` | sql-parser-planner-executor | `table_info`, `table_xinfo`, `index_list`, `index_info`, `foreign_key_list`, `database_list` and `table_list` match the rusqlite oracle on the columns `reference_build_pragma_rows_match_for_supported_surfaces` compares. Open: `PRAGMA index_xinfo` omits the rowid key row (`1\|-1\|\|0\|BINARY\|0`). |
| Full reference-build PRAGMA set | partial | semantic_match | `crates/sql/tests/sqlite_full_parity.rs`: `reference_build_pragmas_are_fully_classified`, `reference_build_pragma_rows_match_for_supported_surfaces`, `accepted_uncompared_pragmas_are_accepted_by_redline`, `explicit_reject_pragmas_are_rejected_by_redline`, `known_gap_pragmas_are_rejected_or_diverge` | sql-parser-planner-executor | Every PRAGMA the bundled SQLite lists is in exactly one class in `crates/sql/tests/sqlite_full_parity.rs`, and each class is tested: row-compared with the oracle, accepted without a comparison, a known gap with a fixture that shows it, or an explicit reject. Open: PRAGMAs in `KNOWN_GAP_REFERENCE_PRAGMAS` are rejected or answer differently; PRAGMAs in `ACCEPTED_UNCOMPARED_REFERENCE_PRAGMAS` answer with RedlineDB's own stored or constant value; no SQLite value is claimed. |

## File Format, Durability, C API, And CLI

| Feature row | Status | Proof | Tests | Owner | Notes |
|---|---|---|---|---|---|
| SQLite database header/pages/btrees/records | not-started | known_divergence | `crates/sql/tests/sqlite_full_parity.rs`: `sqlite_native_file_format_is_not_compatibility_surface` | storage-and-catalog | Current files use RedlineDB-native page/control/WAL formats, not `SQLite format 3`; the parity suite now asserts the header mismatch explicitly. |
| SQLite rollback journal compatibility | not-started | none | none | storage-and-catalog | No SQLite rollback-journal reader/writer exists. |
| SQLite WAL compatibility and recovery | not-started | none | none | storage-and-catalog | RedlineDB has a native group-commit WAL, not SQLite WAL frames. |
| Cross-open RedlineDB-created files with SQLite CLI | not-started | known_divergence | `crates/sql/tests/sqlite_full_parity.rs`: `sqlite_native_file_format_is_not_compatibility_surface` | storage-and-catalog | Requires SQLite file-format writer. |
| Cross-open SQLite-created files with RedlineDB | not-started | known_divergence | `crates/sql/tests/sqlite_full_parity.rs`: `sqlite_native_file_is_not_redline_database_root` | storage-and-catalog | Requires SQLite pager/btree/record reader. |
| Covered `sqlite3_*` open/prepare/step/finalize aliases | pass | semantic_match | `crates/ffi/tests/error_paths.rs`: `prepare_with_null_db_returns_misuse_and_leaves_out_stmt_null`; `crates/ffi/tests/upstream_abi.rs`: `prepare_v3_upstream_order_flags_0_1_2`, `prepare_v3_unsupported_flag_nulls_stmt`; `crates/ffi/tests/value_result.rs`: `null_value_reports_null_type_and_zero_accessors`; `crates/ffi/tests/**`; `contracts/c-abi/redlinedb.h`; `scripts/compatibility/phase2-abi-probe.sh`; `contracts/c-abi/probe/phase2_abi_probe.c` | c-abi | Covered ABI subset only. `crates/ffi/tests/upstream_abi.rs` and the C probe call it through upstream 3.53.1 declarations (prepare_v2/v3 bounds and tails, flags, storage classes, text conversions, `:memory:`); upstream libsqlite3 passes the same probe cases as a control. Header edits require an exception receipt. |
| Broad `sqlite3_*` API surface | partial | semantic_match | `crates/ffi/tests/blob_io.rs`: `open_read_write_close_round_trip`; `crates/ffi/tests/hooks.rs`: `commit_hook_fires_and_can_veto`, `authorizer_returns_decision_code`; `crates/ffi/tests/value_result.rs`: `result_setters_populate_context_slot`; `crates/ffi/src/sqlite3_api/**`; `crates/ffi/tests/**` | c-abi | B1-B5: 36 additional `sqlite3_*` symbols implemented across `sqlite3_api/{result,value,context,udf,collation,blob,hooks,hooks_fire,bind,column,core,exec,meta,stmt}.rs`. Covers UDF context + result family, value extraction, blob I/O, collation registration, per-connection hooks (`commit`, `rollback`, `update`, `trace`, `profile`, `busy_handler`, `set_authorizer`), and the remaining stmt/meta/exec aliases. Symbol-allowlist tests (`crates/ffi/tests/symbol_diff.rs`) enforce the surface. Partial because several hooks exist without SQLite's behaviour: `sqlite3_trace_v2` refuses a callback (`SQLITE_ERROR`) because it delivers no events; `sqlite3_trace`, `sqlite3_profile` and the commit and rollback hooks fire only from `sqlite3_exec`, and a commit hook cannot veto; the busy handler is never called; the authorizer is asked only table-level `SQLITE_SELECT`/`INSERT`/`UPDATE`/`DELETE` when a statement steps (no `SQLITE_READ` per column, DDL, PRAGMA, ATTACH, function or transaction codes), and an invalid return code fails the statement ("authorizer malfunction"). See `docs/security-capabilities.md`. Open: many upstream functions are not exported; `crates/ffi/tests/symbol_allowlist.toml` lists them. |
| CLI one-shot query/stats/backup commands | partial | semantic_match | `crates/cli/tests/dot_commands.rs`: `batch_bail_memory_mode_is_cwd_clean_and_output_exact`; `crates/cli/tests/shellzero_smoke.rs`: `shellzero_select_one_plus_one`, `without_shellzero_select_still_works`; `.jankurai/test-map.json` | cli-shell | One-shot queries (`redlinedb DB SQL`), `stats` and `backup` run in the CLI test lanes listed in `.jankurai/test-map.json`. Corpus cases: 11306, 11314, 11322, 11330, 11338, 11346, 11354, 11362. Open: SQLite shell scripting beyond one-shot query, stats and backup; column, box, table and markdown modes lay a BLOB out as text, so a byte that is not UTF-8 shows as U+FFFD there. |
| SQLite shell dot-command compatibility | partial | semantic_match | `crates/cli/tests/dot_commands.rs`: `dot_tables_lists_user_tables`, `dot_schema_prints_create_statements`, `dot_dump_round_trips_through_sqlite3`, `dot_parameter_set_binds_named_placeholders` | cli-shell | 25 dot-commands are wired through `crates/cli/src/dot/{mod,control,display,io_cmd,schema,parameter}.rs`, including `.fullschema`, `.once FILE` and `.parameter set\|unset\|init\|clear\|list`. Corpus cases: 00115, 00117–00118, 00121, 00124, 00128–00129, 00133–00134, 00140–00141, 00152, 00158–00159, 10182. Open: the official corpus fails the listed dot-command cases byte for byte; each is in `metadata/sqlite_parity/known-failures.json` with its reason. |

<!-- sqlite-feature-matrix:end -->

## Qualification objects

Two narrower lanes sit beside the official SQL/CLI corpus. CI's
`tests (qualification)` shard runs `scripts/qualification/emit-sqlite-qualification.sh`
and uploads the `sqlite-qualification` artifact; nothing in them is typed by
hand.

- `sqlite_rust_values.json`: typed values through the Rust API, compared
  in-process with the SQLite library `rusqlite` bundles
  (`crates/sql/tests/sqlite_full_parity.rs`). It records that library's
  `sqlite_version()`, which is not the 3.53.1 reference shell the official
  corpus uses, and the `rusqlite` and `libsqlite3-sys` versions from
  `Cargo.lock`.
- `sqlite_c_abi.json`: `cargo test -p redlinedb-ffi` and the upstream-header
  probe `scripts/compatibility/phase2-abi-probe.sh`, with each case's result
  and the header, probe and library hashes; the upstream control is recorded
  as skipped when no reference `libsqlite3` was built.

Neither says anything about the SQLite file format; the file-format rows
above are sentinels only.

## Evidence Boundary

Use `just redline-testing-official` or `just sqlite-parity-report-update`.
The canonical `ci_install_redline_testing` builds the included workspace with
`--locked`, stages the runner and its source data, verifies its SHA-256 and version,
and writes the provenance sidecar consumed by the evidence processor. The source
manifest records `local-bin` to distinguish this checkout build from older release
artifacts. This is the normal GitHub CI path after consolidation.

All four declared suites are required: `sqlite_parity`, `sqlite_parity_memory`,
`rql_phase1`, and `beyond_sqlite`. Missing evidence and baseline regressions fail.
Historical release-download support remains for historical report verification;
it is not required to build or validate the current checkout. The retired engine
`sqlite_parity` producer commands remain disabled so evidence has one owner.
