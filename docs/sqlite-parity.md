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
any shell without its build stamp; `ship-gate` checks the pinned manifest as
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
| 00167 | `DOT_UNMODULE_CATALOG` | exit 0 | exit 1, unknown command `unmodule` |
| 00168 | `DOT_CHECK_CATALOG` | exit 0 | exit 1, `no .testcase is active` |
| 00171 | `OPT_HELP` | exit 1 | exit 0; help text on stderr |
| 00178 | `OPT_HTML_MODE` | fragment `<TD>1</TD>` | `<TD>1` then `<TD>x`, no closing tags |
| 00194 | `OPT_IFEXISTS_NEGATIVE_TEMPFILE` | `unknown option: -ifexists` | `-ifexists` refuses the missing file |
| 00199 | `OPT_PAGECACHE` | stdout `1` | a `Page cache size increased ...` line, then `1` |
| 00219, 00220 | `UPDATE_LIMIT_OPTIONAL`, `DELETE_LIMIT_OPTIONAL` | exit 0 with updated rows | exit 1, `near "ORDER": syntax error` (declared shared rejection) |
| 00222 | `OPT_ESCAPE_SYMBOL` | fragment `\n` (RedlineDB's output) | the newline stays unescaped |
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
whose raw result shows the pass. The ledger rows below predate target-side
enforcement: a row can read pass while that list holds an error-message case
for the same feature (10560 for savepoints, 10253 and 10568 for
`PRAGMA query_only`, 10584 for foreign keys). SQ-07 regenerates the ledger from
typed proof.

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

Cases the pinned build cannot express are declared in
`subrepos/redline-testing/metadata/sqlite_parity/declared-deviations.json`
and listed in the report: the `soundex()` cases 11437-11439 and 00219/00220
record the shared rejection, and 10546, written to show that a default build
lacks `median()`, now checks `median()` because the pinned build enables
`SQLITE_ENABLE_PERCENTILE`.

## SQL Surface

| Feature row | Status | Test path | Owner | Notes |
|---|---|---|---|---|
| Basic `SELECT` projection/filter/order | pass | `crates/sql/tests/smoke_select.rs`, `crates/sql/tests/differential_lab.rs` | sql-parser-planner-executor | Covered for scalar values and simple predicates. |
| `INSERT`, `UPDATE`, `DELETE` basics | pass | `crates/sql/tests/smoke_dml.rs`, `crates/bench/compat/**` | sql-parser-planner-executor | Cross-engine SQLLogicTest corpus covers representative DML. |
| `CREATE TABLE`, `DROP TABLE` basics | partial | `crates/bench/compat/**`, `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | The official sqlite_parity corpus covers basic create/drop. This row stays partial because SQLite metadata compatibility is not complete. |
| `CREATE TABLE AS SELECT` | pass | `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | CTAS names, declared types, row order, `IF NOT EXISTS` short-circuiting, and rollback on source-query failure are covered against `rusqlite`. |
| `CREATE INDEX`, `DROP INDEX` basics | pass | `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | Basic B-tree-backed indexes only. |
| `ALTER TABLE RENAME TABLE/COLUMN` | partial | `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | The official sqlite_parity corpus covers rename of tables and columns. This row stays partial because add/drop-column variants are outside that covered subset. |
| `RETURNING` expressions | pass | `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | Insert/update expressions covered. |
| UPSERT `ON CONFLICT DO UPDATE` representative path | partial | `crates/bench/compat/orm/migration.sqlt`, `crates/sql/tests/phase10_sqlc_conflict_matrix.rs` | sql-parser-planner-executor | The official sqlite_parity corpus covers the representative `ON CONFLICT DO UPDATE` path. This row stays partial because the full SQLite conflict matrix is outside that covered subset. |
| Savepoints | pass | `crates/sql/tests/parity_coverage.rs`, `crates/sql/tests/phase10_sqlb.rs` | sql-parser-planner-executor | Nested savepoint behavior has focused coverage. |
| Joins and left joins | pass | `crates/sql/tests/parity_coverage.rs`, `crates/sql/tests/smoke_select.rs`, `crates/sql/tests/differential_lab.rs`, `crates/bench/compat/orm/queries.sqlt` | sql-parser-planner-executor | Explicit join forms are covered. NATURAL JOIN and USING merged-column output semantics are implemented (W9-T9, `72b4107`); corpus cases 10445/10451/10466 pass. Cross joins, self-joins, and multi-way joins have coverage via `differential_lab`. |
| Row-value `IN` subqueries | pass | `crates/sql/tests/parity_coverage.rs`, `crates/sql/tests/differential_lab.rs` | sql-parser-planner-executor | Representative row-value `IN`/`NOT IN` covered. |
| Correlated subqueries | pass | `crates/sql/tests/differential_lab.rs` | sql-parser-planner-executor | A7: thread-local outer-row stack resolves qualified outer-scope references; correlated `EXISTS` / `NOT EXISTS` and correlated scalar subqueries in projection covered against the rusqlite oracle. |
| CTEs (`WITH`, recursive and non-recursive) | pass | `crates/sql/tests/parity_cte.rs` | sql-parser-planner-executor | A3: non-recursive + recursive (UNION / UNION ALL) materialized into thread-local row store; accepts `AS MATERIALIZED` / `AS NOT MATERIALIZED` as no-op hints and supports JOIN against CTE via synthetic TableDef. Iteration cap 10_000. |
| Compound `SELECT` (`UNION`, `INTERSECT`, `EXCEPT`) | pass | `crates/sql/tests/parity_compound_select.rs`, `crates/sql/tests/parity_order_by_ordinal.rs` | sql-parser-planner-executor | A2: UNION / UNION ALL / INTERSECT / EXCEPT all wired through the parser and exec layers; column-arity and type-class mismatches return diagnostic errors that match SQLite's error class. Parameterized compound branches and trailing wrapper `ORDER BY` / `LIMIT` share stable parameter slots, and top-level integer literals in trailing `ORDER BY` resolve as 1-based output-column references (single-branch and compound), matching SQLite. |
| Window functions and frames | pass | `crates/sql/tests/parity_window.rs` | sql-parser-planner-executor | A4: ROW_NUMBER / RANK / DENSE_RANK / NTILE / LAG / LEAD / FIRST_VALUE / LAST_VALUE / NTH_VALUE / PERCENT_RANK / CUME_DIST + aggregate-OVER (SUM/COUNT/AVG/MIN/MAX/TOTAL) with ROWS / RANGE / GROUPS frames. |
| Views | partial | `crates/sql/tests/parity_view.rs` | sql-parser-planner-executor | The official sqlite_parity corpus covers `CREATE [TEMP] VIEW [IF NOT EXISTS]` and `DROP VIEW [IF EXISTS]`. Views persist in the catalog (format_version 4) and expand at FROM-binding time. DML on a view returns a SQLite-style "cannot modify view" error. This row stays partial because cached prepared statements still do not observe fresh rows after data changes (bind-time expansion, not runtime materialization). |
| Triggers | partial | `crates/sql/tests/parity_trigger.rs` | sql-parser-planner-executor | The official sqlite_parity corpus covers `CREATE TRIGGER {BEFORE\|AFTER} {INSERT\|UPDATE [OF col,...]\|DELETE}` plus `DROP TRIGGER`, including `OLD`/`NEW` and `UPDATE OF`. This row stays partial because `INSTEAD OF` on views is outside that covered subset, and `Txn::trigger_depth` caps recursion at 32 in debug rather than SQLite's 1000. |
| Generated columns | pass | `crates/sql/tests/parity_generated_col.rs` | sql-parser-planner-executor | A6 SQL-D: `STORED` and `VIRTUAL` generated columns parse, persist (catalog format_version 7), recompute on INSERT/UPDATE (STORED), and evaluate on read (VIRTUAL). Writes targeting generated columns are rejected with a SQLite-class error. |
| Partial indexes | pass | `crates/sql/tests/parity_partial_index.rs` | sql-parser-planner-executor | A6 SQL-D: `CREATE INDEX ... WHERE <predicate>` persists the predicate as verbatim SQL on `IndexDef.predicate_sql`; DML re-parses and evaluates the predicate per row (`crates/sql/src/exec/index_predicate.rs`) so partial indexes track in/out membership precisely, and reads only use the index when the query predicate implies the index predicate (`crates/sql/src/exec/index_partial.rs`). |
| Expression indexes | pass | `crates/sql/tests/parity_expr_index.rs` | sql-parser-planner-executor | A6 SQL-D: `CREATE INDEX ... ON t(expr(col))` stashes the expression SQL on `IndexKeySource::Expression`; DML re-evaluates the expression to compute the index key, and UPDATE re-emits when any referenced column changes. |
| Foreign keys | pass | `crates/sql/tests/parity_fk_enforce.rs`, `crates/sql/tests/phase10_sqld_fk.rs` | sql-parser-planner-executor | A6-fk: PRAGMA-gated enforcement on INSERT/UPDATE/DELETE; ON DELETE/UPDATE {NO ACTION, RESTRICT, CASCADE, SET NULL, SET DEFAULT}; DEFERRABLE INITIALLY DEFERRED checked at COMMIT. Cascade depth bounded. |
| ATTACH / DETACH | partial | `crates/sql/tests/parity_attach.rs` | sql-parser-planner-executor | Corpus cases 10001–10021 pass for cross-database `SELECT`, JOIN, `INSERT ... SELECT`, `UPDATE`, `DELETE`, and `PRAGMA` routing. This row stays partial because cross-database `INSERT`/`UPDATE`/`DELETE` without a `SELECT` subquery, and cross-database transaction semantics, are outside that covered subset. |

## Expressions And Functions

| Feature row | Status | Test path | Owner | Notes |
|---|---|---|---|---|
| NULL comparison and `IN`/`NOT IN` edge cases | pass | `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | SQLite three-valued logic has focused coverage. |
| Core string scalars (`substr`, `trim`, `instr`, `replace`, case conversion, length) | pass | `crates/sql/tests/parity_scalar_funcs.rs`, `crates/sql/tests/differential_lab.rs` | sql-parser-planner-executor | Representative SQLite behavior covered. |
| Formatting and conditional scalars (`printf`, `format`, `iif`, `sign`) | pass | `crates/sql/tests/parity_scalar_funcs.rs`, `crates/sql/tests/differential_lab.rs` | sql-parser-planner-executor | Does not imply exhaustive SQLite format coverage. |
| Blob/character helpers (`zeroblob`, `randomblob`, `char`, `unicode`) | pass | `crates/sql/tests/parity_scalar_funcs.rs` | sql-parser-planner-executor | Randomness is shape-tested, not value-matched. |
| Aggregate functions (`count`, `sum`, `total`, `min`, `max`) | pass | `crates/sql/tests/parity_agg_funcs.rs`, `crates/sql/tests/differential_lab.rs` | sql-parser-planner-executor | Representative grouped and NULL behavior covered. |
| JSON aggregate functions | pass | `crates/sql/tests/parity_agg_funcs.rs` | phase10-json1-surface | JSON aggregate rows covered. |
| JSON scalar functions | pass | `crates/sql/tests/phase10_j1_compat.rs`, `crates/sql/tests/parity_json1.rs`, `crates/sql/src/json/scalar.rs` | phase10-json1-surface | `parity_json1.rs` runs a rusqlite-oracle differential pass over `json()` / `json_array[_length]` / `json_object` / `json_extract` / `json_type` / `json_valid` / `json_quote` / `json_set` / `json_insert` / `json_replace` / `json_remove` / `json_patch` and the `->` / `->>` arrow operators. |
| Date/time functions | pass | `crates/sql/tests/phase10_sqld_datetime.rs`, `crates/sql/src/datetime.rs` | phase10-datetime | A8: `date()` / `time()` / `datetime()` / `julianday()` / `unixepoch()` / `strftime()` accept the SQLite time-string formats and modifiers (`'now'`, `'+/-N {days,hours,...}'`, `'start of {day,month,year}'`, `'weekday N'`, `'utc'`, `'localtime'`) through the shared modifier pipeline in `crates/sql/src/datetime/modifiers.rs`; differential parity against the rusqlite oracle is wired and green. |
| Collations | partial | `crates/sql/tests/phase10_sqld_collation.rs`, `crates/sql/tests/phase10_sqld_collation_unique_index.rs`, `crates/sql/tests/parity_order_by_ordinal.rs` | phase10-collations | Explicit `ORDER BY ... COLLATE NOCASE/RTRIM/BINARY` and corpus case 10340 (`CREATE UNIQUE INDEX ... COLLATE NOCASE`) pass. This row stays partial because implicit column-affinity collation and LIKE-with-NOCASE index optimization are outside that covered subset. |
| User-defined SQL functions/collations | pass | `crates/ffi/tests/udf_register.rs`, `crates/ffi/tests/collation_register.rs` | c-abi | B2: `sqlite3_create_function{,_v2,16}` registers scalar UDFs; `sqlite3_create_collation*` + `sqlite3_collation_needed` register collation callbacks. B4 dispatch path routes the registered C callbacks through the SQL evaluator so user-defined functions and collations are invoked end-to-end from prepared statements. |

## PRAGMAs

| Feature row | Status | Test path | Owner | Notes |
|---|---|---|---|---|
| `PRAGMA integrity_check` / `quick_check` | pass | `crates/sql/tests/parity_coverage.rs` | sql-parser-planner-executor | Current checks are RedlineDB-native integrity summaries. |
| `PRAGMA auto_vacuum` (rejected) | rejects-by-design | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | Previously returned a fabricated `0` row; now `UnsupportedSql`. |
| `PRAGMA wal_checkpoint(MODE)` (rejected) | rejects-by-design | `crates/sql/tests/parity_pragma_tv.rs` | storage-and-catalog | Previously returned fabricated `(busy, log, checkpointed)`; RedlineDB has no WAL journal so the PRAGMA is rejected naming WAL as the missing prerequisite. |
| `PRAGMA journal_mode` (memory/off/delete/wal) | pass | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | Round-trip on the supported subset plus RedlineDB's WAL-style `wal` mode; `truncate` and `persist` are out of scope because SQLite page-format WAL semantics are not implemented. |
| `PRAGMA synchronous` | pass | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | Accepts `OFF`/`NORMAL`/`FULL`/`EXTRA` plus integer aliases; stored for read-back. |
| `PRAGMA temp_store` | pass | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | Accepts `DEFAULT`/`FILE`/`MEMORY` plus integer aliases. |
| `PRAGMA cache_size` | pass | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | Signed integer round-trip (negative = KiB, positive = pages). |
| `PRAGMA query_only` | pass | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | When ON, blocks every write-side statement with a message containing `query_only`. |
| Unknown PRAGMA names | rejects-by-design | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | All unsupported PRAGMAs now return `UnsupportedSql` naming the PRAGMA. |
| `PRAGMA foreign_keys` | pass | `crates/sql/tests/parity_fk_enforce.rs`, `crates/sql/tests/phase10_sqld_fk.rs` | sql-parser-planner-executor | A6: per-connection toggle now gates the FK enforcement layer end-to-end; OFF skips every check (matches SQLite's bundled default). |
| Table-valued PRAGMAs | pass | `crates/sql/tests/parity_pragma_tv.rs` | sql-parser-planner-executor | C2: `PRAGMA table_info(...)`, `PRAGMA index_list(...)`, `PRAGMA index_info(...)`, `PRAGMA foreign_key_list(...)` and the rest of the table-valued PRAGMA family produce row sets with column names and ordering that match the rusqlite oracle. |
| Full reference-build PRAGMA set | partial | `crates/sql/tests/sqlite_full_parity.rs` | sql-parser-planner-executor | The official sqlite_parity corpus compares `PRAGMA compile_options` and `PRAGMA pragma_list` for the supported subset. This row stays partial because intentional rejects (`auto_vacuum`, `wal_checkpoint(MODE)`, unknown names) remain explicit sentinels outside that subset. |

## File Format, Durability, C API, And CLI

| Feature row | Status | Test path | Owner | Notes |
|---|---|---|---|---|
| SQLite database header/pages/btrees/records | not-started | `crates/sql/tests/sqlite_full_parity.rs` sentinel only | storage-and-catalog | Current files use RedlineDB-native page/control/WAL formats, not `SQLite format 3`; the parity suite now asserts the header mismatch explicitly. |
| SQLite rollback journal compatibility | not-started | none | storage-and-catalog | No SQLite rollback-journal reader/writer exists. |
| SQLite WAL compatibility and recovery | not-started | none | storage-and-catalog | RedlineDB has a native group-commit WAL, not SQLite WAL frames. |
| Cross-open RedlineDB-created files with SQLite CLI | not-started | none | storage-and-catalog | Requires SQLite file-format writer. |
| Cross-open SQLite-created files with RedlineDB | not-started | none | storage-and-catalog | Requires SQLite pager/btree/record reader. |
| Covered `sqlite3_*` open/prepare/step/finalize aliases | pass | `crates/ffi/tests/**`, `contracts/c-abi/redlinedb.h` | c-abi | Covered ABI subset only. Header edits require an exception receipt. |
| Broad `sqlite3_*` API surface | pass | `crates/ffi/src/sqlite3_api/**`, `crates/ffi/tests/**` | c-abi | B1-B5: 36 additional `sqlite3_*` symbols implemented across `sqlite3_api/{result,value,context,udf,collation,blob,hooks,hooks_fire,bind,column,core,exec,meta,stmt}.rs`. Covers UDF context + result family, value extraction, blob I/O, collation registration, per-connection hooks (`commit`, `rollback`, `update`, `trace`, `profile`, `busy_handler`, `set_authorizer`), and the remaining stmt/meta/exec aliases. Symbol-allowlist tests (`crates/ffi/tests/symbol_diff.rs`) enforce the surface. |
| CLI one-shot query/stats/backup commands | partial | `crates/cli` smoke lanes via `.jankurai/test-map.json` | cli-shell | This row stays partial because SQLite shell scripting beyond those one-shot query, stats, and backup paths is outside that covered subset. |
| SQLite shell dot-command compatibility | partial | `crates/cli/tests/dot_commands.rs` | cli-shell | 25 dot-commands wired through `crates/cli/src/dot/{mod,control,display,io_cmd,schema,parameter}.rs`; adds `.fullschema` (schema + `sqlite_master`), `.once FILE` (one-shot redirect plumbed through `run_query_with_state`), and `.parameter set|unset|init|clear|list` (bindings applied via `bind_named` on the next statement). This row is partial because the official corpus, comparing stdout byte for byte, fails these dot-command cases: 00115 `.schema`, 00117 `.dump`, 00118 `.fullschema`, 00121 `.parameter list`, 00124 `.bail off`, 00128 `.dbconfig`, 00129 `.connection`, 00133 `.auth`, 00134 `.crlf`, 00140 `.expert`, 00141 `.sha3sum`, 00152 `.clone`, 00158 `.shell`, 00159 `.system` and 10182 `.show`. Each is listed with its reason in `metadata/sqlite_parity/known-failures.json`. |

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
