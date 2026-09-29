# jankurai Repo Score

- Standard: `jankurai`
- Auditor: `1.6.11`
- Schema: `1.9.0`
- Paper edition: `2026.05-ed8`
- Target stack ID: `rust-ts-vite-react-postgres-bounded-python`
- Target stack: `Rust core + TypeScript/React/Vite + PostgreSQL + generated contracts + exception-only Python AI/data service`
- Repo: `.`
- Run ID: `1790674548`
- Started at: `1790674548`
- Elapsed: `29327` ms
- Scope: `full`
- Raw score: `86`
- Final score: `86`
- Decision: `advisory`
- Minimum score: `85`
- Caps applied: `none`

## Hard Rule Caps

| Rule | Max Score | Applied |
| --- | ---: | --- |
| `no-root-agent-instructions` | 75 | no |
| `no-one-command-setup-or-validation` | 70 | no |
| `no-deterministic-fast-lane` | 65 | no |
| `no-security-lane-on-high-risk-repo` | 60 | no |
| `generated-contracts-or-public-api-drift-untested` | 80 | no |
| `python-direct-product-truth-or-db-ownership` | 72 | no |
| `no-secret-or-dependency-scanning-in-ci` | 78 | no |
| `no-jankurai-audit-lane-in-ci` | 82 | no |
| `jankurai-required-tool-ci-evidence-gap` | 88 | no |
| `non-optimal-product-language-found` | 74 | no |
| `too-much-python-in-product-surface` | 72 | no |
| `boundary-reclassification-evidence-gap` | 72 | no |
| `vibe-placeholders-in-product-code` | 68 | no |
| `fallback-soup-in-product-code` | 70 | no |
| `future-hostile-dead-language-in-product-code` | 64 | no |
| `severe-duplication-in-product-code` | 70 | no |
| `generated-zone-mutation-risk` | 76 | no |
| `direct-db-access-from-wrong-layer` | 66 | no |
| `missing-web-e2e-lane` | 82 | no |
| `missing-rendered-ux-qa-lane` | 84 | no |
| `prompt-injection-risk` | 78 | no |
| `overbroad-agent-agency` | 65 | no |
| `secret-like-content-detected` | 60 | no |
| `false-green-test-risk` | 76 | no |
| `destructive-migration-risk` | 70 | no |
| `authz-or-data-isolation-gap` | 78 | no |
| `input-boundary-gap` | 78 | no |
| `agent-tool-supply-chain-gap` | 78 | no |
| `release-readiness-gap` | 80 | no |
| `missing-rust-property-or-integration-tests` | 82 | no |
| `no-agent-friendly-exception-pattern` | 76 | no |
| `missing-agent-readable-docs` | 80 | no |
| `streaming-runtime-drift` | 78 | no |
| `rust-bad-behavior` | 72 | no |
| `sql-bad-behavior` | 72 | no |
| `typescript-bad-behavior` | 72 | no |
| `docker-bad-behavior` | 72 | no |
| `python-bad-behavior` | 72 | no |
| `ci-bad-behavior` | 70 | no |
| `git-bad-behavior` | 70 | no |
| `gittools-bad-behavior` | 70 | no |
| `release-bad-behavior` | 70 | no |
| `web-security-bad-behavior` | 68 | no |
| `repo-rot-bad-behavior` | 88 | no |
| `comment-hygiene-dangerous-residue` | 72 | no |
| `ci-local-parity` | 70 | no |

## Copy-Code Redundancy

- Status: `review` hard=`0` warning=`197` files=`562`
- Policy: min-lines=`10` min-tokens=`100` max-findings=`50` include-tests=`false` strict=`false`
- Duplicate volume: lines=`426` tokens=`1320` bytes=`12044`

- Notes:
  - hard classes are limited to exact active-source file matches and substantial exact same-name units
  - warning classes include same-body different-name units and token/block duplication
  - tests, fixtures, stories, config, Docker, and migrations are omitted unless --include-tests is set
  - showing the top 50 classes and omitting 147 lower-ranked classes

| Kind | Severity | Language | Lines | Tokens | Instances | Reason |
| --- | --- | --- | ---: | ---: | --- | --- |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 9 | `crates/sql/src/exec/json_tv.rs:228-233, crates/sql/src/exec/json_tv.rs:283-288, crates/sql/src/exec/json_tv.rs:399-404, crates/sql/src/exec/json_tv.rs:433-438, crates/sql/src/exec/json_tv.rs:467-472, crates/sql/src/exec/json_tv.rs:504-509, crates/sql/src/exec/json_tv.rs:541-546, crates/sql/src/exec/json_tv.rs:578-583, crates/sql/src/exec/json_tv.rs:616-621, crates/sql/src/exec/pragma_tv.rs:220-225, crates/sql/src/exec/pragma_tv.rs:243-248, crates/sql/src/exec/pragma_tv.rs:282-287, crates/sql/src/exec/pragma_tv.rs:370-375` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 13 | 42 | `crates/sql/src/exec/cross_db.rs:199-212, crates/sql/src/exec/cte.rs:203-216, crates/sql/src/exec/view.rs:202-215` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 24 | 84 | `crates/sql/src/parser/pg_fn_stmt.rs:246-270, crates/sql/src/pg_pl.rs:1434-1458` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 11 | 42 | `crates/sql/src/parser/pg_fn_stmt.rs:272-283, crates/sql/src/pg_pl.rs:1482-1493, crates/sql/src/pg_pub.rs:205-216` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 2 | `crates/bench/src/score_policy.rs:180-181, crates/bench/src/score_policy.rs:188-189, crates/kernel/src/catalog/record.rs:152-153, crates/kernel/src/catalog/stats/wire.rs:162-163, crates/kernel/src/catalog/stats/wire.rs:173-174, crates/kernel/src/catalog/store.rs:1172-1173, crates/kernel/src/catalog/store.rs:1182-1183, crates/kernel/src/catalog/store.rs:1192-1193, crates/kernel/src/catalog/store.rs:1202-1203, crates/kernel/src/catalog/store.rs:1212-1213, crates/kernel/src/catalog/store.rs:1235-1236, crates/redlinedb-sqlx/src/bridge/runtime.rs:390-391, crates/redlinedb-sqlx/src/bridge/runtime.rs:487-488, crates/redlinedb/src/value_conv.rs:261-262, crates/sql/src/exec/expr/json_dispatch.rs:821-822, crates/sql/src/exec/expr/scalar/pg_session.rs:183-184, crates/sql/src/exec/expr/scalar/pg_session.rs:294-295, crates/sql/src/exec/index_access.rs:1193-1194, crates/sql/src/exec/json_tv.rs:241-242, crates/sql/src/json/scalar.rs:134-135, crates/sql/src/json/scalar.rs:151-152, crates/sql/src/json/scalar.rs:181-182, crates/sql/src/json/scalar.rs:643-644` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 7 | 16 | `crates/cli/src/render.rs:473-480, crates/cli/src/render.rs:592-599, crates/cli/src/render.rs:653-660, crates/cli/src/render.rs:727-734` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 9 | `crates/sql/src/exec/pragma_tv.rs:76-81, crates/sql/src/exec/pragma_tv.rs:108-113, crates/sql/src/exec/pragma_tv.rs:134-139, crates/sql/src/exec/pragma_tv.rs:429-434, crates/sql/src/exec/pragma_tv.rs:463-468` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 16 | 51 | `crates/sql/src/matview.rs:322-338, crates/sql/src/parser/rewrite/pg_empty.rs:44-60` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 16 | 37 | `crates/sql/src/exec/agg/select.rs:85-101, crates/sql/src/planner/access/projection.rs:153-169` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 2 | 5 | `crates/kernel/src/json/path_bytecode.rs:138-140, crates/sql/src/json/jsonb.rs:1062-1064, crates/sql/src/matview.rs:340-342, crates/sql/src/parser/rewrite/pg_ddl.rs:349-351, crates/sql/src/parser/rewrite/pg_ddl.rs:1368-1370, crates/sql/src/parser/rewrite/pg_empty.rs:62-64, crates/sql/src/pg_fn.rs:220-222, crates/sql/src/pg_pub.rs:201-203` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 13 | 36 | `crates/sql/src/pg_fn.rs:168-181, crates/sql/src/pg_pl.rs:1350-1363` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 2 | `crates/bench/src/config.rs:438-439, crates/bench/src/config.rs:459-460, crates/bench/src/durability_evidence/receipt.rs:67-68, crates/redlinedb/src/value.rs:43-44, crates/redlinedb/src/value.rs:57-58, crates/redlinedb/src/value.rs:64-65, crates/redlinedb/src/value.rs:71-72, crates/redlinedb/src/value.rs:78-79, crates/sql/src/exec/expr/scalar/row/model.rs:69-70, crates/sql/src/exec/expr/scalar/row/model.rs:83-84, crates/sql/src/exec/expr/scalar/row/model.rs:94-95, crates/sql/src/statement.rs:496-497, crates/sql/src/statement.rs:797-798` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 11 | 57 | `crates/sql/src/datetime/format.rs:75-86, crates/sql/src/datetime/modifiers.rs:193-204` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 9 | `crates/sql/src/exec/pragma_tv.rs:170-175, crates/sql/src/exec/table_valued.rs:77-82, crates/sql/src/exec/table_valued.rs:141-146` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 2 | 3 | `crates/cli/src/dot/control.rs:106-108, crates/cli/src/dot/control.rs:110-112, crates/cli/src/dot/control.rs:114-116, crates/cli/src/dot/control.rs:127-129, crates/cli/src/dot/control.rs:281-283, crates/cli/src/dot/control.rs:381-383` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 4 | `crates/sql/src/parser/matview_stmt.rs:169-170, crates/sql/src/parser/matview_stmt.rs:202-203, crates/sql/src/parser/pg_fn_stmt.rs:285-286, crates/sql/src/pg_pl.rs:1495-1496, crates/sql/src/pg_pub.rs:218-219, crates/sql/src/pg_search.rs:322-323, crates/sql/src/pg_search.rs:338-339, crates/sql/src/virtual_module.rs:198-199, crates/sql/src/virtual_module.rs:211-212` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 8 | 17 | `crates/kernel/src/catalog/key.rs:191-199, crates/kernel/src/catalog/key_epoch.rs:78-86` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 8 | 17 | `crates/sql/src/numeric.rs:115-123, crates/sql/src/numeric/text_number.rs:171-179` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 2 | 1 | `crates/kernel/src/failpoints/mod.rs:65-67, crates/kernel/src/failpoints/mod.rs:109-111, crates/kernel/src/failpoints/mod.rs:133-135, crates/kernel/src/storage/numa.rs:59-61, crates/kernel/src/storage/numa.rs:64-66` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 7 | 25 | `crates/sql/src/pg_pl.rs:1392-1399, crates/sql/src/pg_pub.rs:192-199` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 7 | 12 | `crates/kernel/src/engine/page_heap/policy.rs:78-85, crates/kernel/src/engine/page_heap/policy.rs:108-115` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 0 | `crates/kernel/src/catalog/ddl.rs:319-319, crates/kernel/src/failpoints/mod.rs:41-42, crates/kernel/src/integrity/equivalence.rs:218-218, crates/kernel/src/integrity/page_csum.rs:107-107, crates/redlinedb-sqlx/src/bridge/options.rs:253-254, crates/redlinedb-sqlx/src/bridge/runtime.rs:127-128, crates/sql/src/connection/session.rs:1417-1417, crates/sql/src/exec/merge.rs:285-285` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 6 | `crates/redlinedb-sqlx/src/bridge/options.rs:223-224, crates/redlinedb-sqlx/src/bridge/runtime.rs:52-53, crates/redlinedb-sqlx/src/bridge/runtime.rs:57-58, crates/redlinedb-sqlx/src/bridge/runtime.rs:81-82, crates/redlinedb-sqlx/src/bridge/runtime.rs:96-97, crates/redlinedb-sqlx/src/bridge/runtime.rs:111-112, crates/redlinedb-sqlx/src/bridge/runtime.rs:201-202` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 4 | `crates/kernel/src/catalog/collation.rs:210-211, crates/kernel/src/catalog/store.rs:700-701, crates/sql/src/parser/add_column_sql.rs:108-109, crates/sql/src/parser/rewrite/sqlite_shape.rs:1262-1263, crates/sql/src/parser/rewrite/sqlite_shape.rs:1361-1362, crates/sql/src/parser/split.rs:53-54, crates/sql/src/parser/split.rs:258-259` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 6 | 20 | `crates/sql/src/exec/agg/select.rs:9-15, crates/sql/src/planner/access/projection.rs:94-100` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 6 | 15 | `crates/bench/src/durability_evidence/readback/parse.rs:196-202, crates/bench/src/recover/observe.rs:207-213` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 2 | 5 | `crates/sql/src/matview.rs:340-342, crates/sql/src/parser/rewrite/pg_empty.rs:62-64, crates/sql/src/pg_fn.rs:220-222, crates/sql/src/pg_pub.rs:201-203` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 6 | 12 | `crates/sql/src/exec/policy.rs:32-38, crates/sql/src/exec/policy.rs:57-63` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 3 | 5 | `crates/sql/src/sqlite_errors.rs:213-216, crates/sql/src/sqlite_errors.rs:229-232, crates/sql/src/sqlite_errors.rs:243-246` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 24 | `crates/bench/src/durability_evidence/readback/parse.rs:189-194, crates/bench/src/recover/observe.rs:215-220` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 1 | 4 | `crates/sql/src/parser/matview_stmt.rs:169-170, crates/sql/src/parser/pg_fn_stmt.rs:285-286, crates/sql/src/pg_pl.rs:1495-1496, crates/sql/src/pg_pub.rs:218-219, crates/sql/src/pg_search.rs:322-323, crates/sql/src/virtual_module.rs:211-212` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 3 | `crates/sql/src/exec/expr/json_dispatch.rs:716-717, crates/sql/src/exec/expr/json_dispatch.rs:745-746, crates/sql/src/exec/expr/json_dispatch.rs:775-776, crates/sql/src/exec/expr/json_dispatch.rs:1022-1023, crates/sql/src/exec/expr/json_dispatch.rs:1035-1036, crates/sql/src/exec/expr/json_dispatch.rs:1049-1050` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 5 | 15 | `crates/sql/src/exec/mod.rs:1772-1777, crates/sql/src/exec/mod.rs:1787-1792` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 13 | `crates/sql/src/exec/agg/select.rs:103-108, crates/sql/src/planner/access/projection.rs:171-176` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 5 | 12 | `crates/cli/src/render.rs:1000-1005, crates/cli/src/render.rs:1020-1025` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 5 | 10 | `crates/sql/src/exec/join_probe.rs:123-128, crates/sql/src/planner/helpers.rs:185-190` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 5 | 10 | `crates/cli/src/render.rs:460-465, crates/cli/src/render.rs:550-555` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 8 | `crates/sql/src/exec/expr/affinity.rs:47-52, crates/sql/src/exec/fk_affinity.rs:27-32` | `same-name semantic unit copied across multiple files` |
| `ExactUnitSameName` | `Warning` | `rust` | 5 | 6 | `crates/sql/src/pg_pl.rs:205-210, crates/sql/src/pg_pub.rs:181-186` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 1 | `crates/bench/src/sqlite_parity/engine.rs:172-173, crates/redlinedb-sqlx/src/driver.rs:216-217, crates/redlinedb/src/iter.rs:93-94, crates/sql/src/exec/expr/predicate.rs:266-267, crates/sql/src/exec/expr/program.rs:380-381, crates/sql/src/exec/morsel/hash_agg.rs:227-228` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 4 | `crates/sql/src/exec/morsel/arena.rs:77-78, crates/sql/src/exec/morsel/arena.rs:103-104, crates/sql/src/exec/morsel/builder.rs:160-161, crates/sql/src/exec/morsel/column.rs:74-75, crates/sql/src/exec/morsel/scan.rs:144-145` | `same body appears under different names across files` |
| `ExactUnitSameName` | `Warning` | `rust` | 4 | 13 | `crates/kernel/src/index/locks.rs:200-204, crates/sql/src/session.rs:641-645` | `same-name semantic unit copied across multiple files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 4 | 12 | `crates/sql/src/exec/expr/json_dispatch.rs:62-66, crates/sql/src/exec/expr/json_dispatch.rs:878-882` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 2 | 5 | `crates/kernel/src/format/bytes.rs:44-46, crates/kernel/src/format/bytes.rs:49-51, crates/kernel/src/format/bytes.rs:54-56` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 4 | 9 | `crates/sql/src/exec/expr/scalar/row/lookup.rs:68-72, crates/sql/src/exec/expr/scalar/row/lookup.rs:137-141` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 1 | 2 | `crates/sql/src/exec/expr/coerce/collation_scope.rs:125-126, crates/sql/src/exec/expr/predicate.rs:248-249, crates/sql/src/parser/order_by.rs:197-198, crates/sql/src/parser/order_by.rs:209-210, crates/sql/src/parser/order_by.rs:226-227` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 2 | 3 | `crates/kernel/src/format/page.rs:102-104, crates/kernel/src/storage/control.rs:255-257, crates/kernel/src/storage/tx_status_checkpoint.rs:190-192` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 4 | 5 | `crates/redlinedb/src/connection.rs:181-185, crates/redlinedb/src/connection.rs:196-200` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 2 | 1 | `crates/bench/src/certify/scheduler/dispatch.rs:196-198, crates/bench/src/durability_evidence/session/child.rs:58-60, crates/kernel/src/engine/runtime/commit.rs:35-37` | `same body appears under different names across files` |
| `ExactUnitDifferentName` | `Warning` | `rust` | 3 | 16 | `crates/sql/src/exec/expr/scalar/value.rs:556-559, crates/sql/src/exec/expr/scalar/value.rs:715-718` | `same body appears under different names across files` |

## Dimensions

| Dimension | Weight | Score | Weighted | Evidence |
| --- | ---: | ---: | ---: | --- |
| Ownership and navigation surface | 13 | 100 | 13.00 | root `AGENTS.md` present; `CODEOWNERS` present |
| Contract and boundary integrity | 13 | 98 | 12.74 | contract surface found; generated contract artifacts found |
| Proof lanes and test routing | 12 | 100 | 12.00 | one-command setup/validation lane found; deterministic fast lane found |
| Security and supply-chain posture | 12 | 70 | 8.40 | lockfile present; secret or dependency scan tooling found |
| Code shape and semantic surface | 12 | 45 | 5.40 | largest authored code file: crates/sql/src/exec/mod.rs (1982 LOC); code file exceeds 500 LOC |
| Data truth and workflow safety | 8 | 100 | 8.00 | database surface present; structured db boundary manifest present |
| Observability and repair evidence | 8 | 98 | 7.84 | observability libraries or patterns found; diagnostic shaping hints found |
| Context economy and agent instructions | 7 | 100 | 7.00 | root `AGENTS.md` present; root `AGENTS.md` stays short |
| Jankurai tool adoption and CI replacement | 7 | 76 | 5.32 | control-plane files present; applicable=16 |
| Python containment and polyglot hygiene | 4 | 100 | 4.00 | no Python files in scope |
| Build speed signals | 4 | 70 | 2.80 | build acceleration markers found; targeted test/build commands found |

## Reference Profile Structure

- Applicable cells: `8` canonical=`8` noncanonical=`0` guidance missing=`0`

| Cell | Status | Canonical | Detected | Aliases | Guidance | Owner | Proof lane | Agent fix |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `web` | `not_applicable` | `apps/web/` | `-` | `frontend/, ui/, packages/web/, packages/ui/` | `not_required` | `apps/web` | `rendered UX / Playwright` | `no action` |
| `api` | `canonical` | `apps/api/` | `apps/api` | `api/, server/, backend/` | `present` | `apps/api` | `edge handler / contract tests` | `keep `apps/api/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `domain` | `canonical` | `crates/domain/` | `crates/domain` | `domain/, core/` | `present` | `crates/domain` | `unit / property tests` | `keep `crates/domain/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `application` | `canonical` | `crates/application/` | `crates/application` | `application/, usecases/, use-cases/` | `present` | `crates/application` | `use-case / authz tests` | `keep `crates/application/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `adapters` | `canonical` | `crates/adapters/` | `crates/adapters` | `adapters/, infra/, integrations/` | `present` | `crates/adapters` | `adapter integration tests` | `keep `crates/adapters/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `workers` | `canonical` | `crates/workers/` | `crates/workers` | `workers/, jobs/, scheduler/, queue/` | `present` | `crates/workers` | `workflow / replay tests` | `keep `crates/workers/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `contracts` | `canonical` | `contracts/` | `contracts` | `openapi/, protobuf/, json-schema/, generated/` | `present` | `contracts` | `generation / drift checks` | `keep `contracts/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `db` | `canonical` | `db/` | `db` | `migrations/, constraints/, sql/` | `present` | `db` | `migration / constraint tests` | `keep `db/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |
| `python-ai` | `not_applicable` | `python/ai-service/` | `-` | `python/, ai-service/, evals/, embeddings/, model/` | `not_required` | `python/ai-service` | `eval / contract tests` | `no action` |
| `ops` | `canonical` | `ops/` | `.github, .github/workflows, ops` | `.github/, .github/workflows/, ci/, release/, observability/, security/` | `present` | `ops` | `security lane / workflow lint` | `keep `ops/AGENTS.md` aligned with owns / forbidden / proof lane guidance` |

## Rendered UX QA

- Web surface: `false`
- Layered UX lane: `true`
- Missing: `none`

## Tool Adoption

- Control plane present: `true`
- Applicable tools: `16`
- Configured: `16`
- CI evidence: `11`
- Artifact verified: `8`
- Replaced count: `11`
- Missing CI evidence: `proof-routing, proofbind, proofmark-rust, copy-code, ci-bad-behavior, git-bad-behavior, release-bad-behavior, rust-witness`

| Tool | Category | Mode | Status | Replaced | Artifacts |
| --- | --- | --- | --- | --- | --- |
| `audit-ci` | `audit` | `auto` | `artifact_verified` | `manual repo scoring, ad hoc score gates` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |
| `proof-routing` | `proof` | `auto` | `ci_evidence` | `ad hoc proof lane selection, manual proof receipts` | `.jankurai/repo-score.json, .jankurai/repo-score.md, target/jankurai/repair-queue.jsonl` |
| `proofbind` | `proof` | `auto` | `configured` | `manual changed-surface routing, ad hoc proof obligation lists` | `target/jankurai/proofbind/surface-witness.json, target/jankurai/proofbind/obligations.json` |
| `proofmark-rust` | `proof` | `auto` | `ci_evidence` | `line-only coverage review, manual in-diff mutation review` | `target/jankurai/proofmark/proofmark-receipt.json, target/jankurai/proofmark/proof-receipt.json` |
| `copy-code` | `audit` | `auto` | `configured` | `ad hoc copy-code review, manual duplication triage` | `target/jankurai/copy-code.json, target/jankurai/copy-code.md` |
| `security` | `security` | `auto` | `artifact_verified` | `gitleaks, dependency review, SBOM/provenance` | `target/jankurai/security/evidence.json` |
| `ci-bad-behavior` | `security` | `auto` | `configured` | `mutable workflow refs, secret echo/debug workflow checks, non-blocking security scans` | `target/jankurai/language-bad-behavior.log` |
| `git-bad-behavior` | `audit` | `auto` | `configured` | `destructive git automation, force-push release scripts, hidden stash-based state` | `target/jankurai/language-bad-behavior.log` |
| `release-bad-behavior` | `release` | `auto` | `configured` | `manual release checklist, ad hoc tag and artifact review, manual provenance review` | `target/jankurai/language-bad-behavior.log` |
| `ux-qa` | `ux` | `auto` | `not_applicable` | `playwright, axe-core, visual baselines` | `target/jankurai/ux-qa.json` |
| `db-migration-analyze` | `db` | `auto` | `not_applicable` | `manual migration review` | `target/jankurai/migration-report.json` |
| `contract-drift` | `contract` | `auto` | `artifact_verified` | `handwritten contract drift checks, openapi diff` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |
| `rust-witness` | `rust` | `auto` | `ci_evidence` | `manual witness graphing` | `target/jankurai/rust/witness-graph.json` |
| `vibe-coverage` | `audit` | `auto` | `not_applicable` | `manual vibe-coding coverage spreadsheet` | `target/jankurai/vibe-coverage.json, target/jankurai/vibe-coverage.md` |
| `coverage-evidence` | `proof` | `auto` | `not_applicable` | `manual coverage report review, ad hoc mutation survivor review` | `target/jankurai/coverage/coverage-audit.json, target/jankurai/coverage/coverage-audit.md` |
| `authz-matrix` | `security` | `auto` | `artifact_verified` | `manual authz matrix review` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |
| `input-boundary` | `security` | `auto` | `artifact_verified` | `manual unsafe sink review` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |
| `agent-tool-supply` | `security` | `auto` | `artifact_verified` | `manual MCP/tool trust review` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |
| `release-readiness` | `release` | `auto` | `artifact_verified` | `manual launch checklist` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |
| `cost-budget` | `release` | `auto` | `artifact_verified` | `manual spend review` | `.jankurai/repo-score.json, .jankurai/repo-score.md` |

## Boundary manifest (ingested)

- Path: `agent/boundaries.toml`
- Stack: `rust-ts-vite-react-postgres-bounded-python` · version: `0.4.0`
- Queue path counts — adapter: `2`, event_contract: `1`, generated_type: `1`, client_marker: `7`, streaming_exception: `1`
- Content fingerprint: `sha256:39f4f2fc80401bd10b3db201e1a8c065a5ba7a77567cf9bd9f3b6069eb40e8be`

## Boundary Reclassifications

No audited runtime boundary reclassifications declared.

## Findings

1. `medium` `shape` `.`
   Rule: `HLT-001-DEAD-MARKER`
   Check: `HLT-001-DEAD-MARKER:shape` `soft` confidence `0.76`
   Route: TLR `Entropy`, lane `fast`, owner `tools`
   Docs: `docs/audit-rubric.md#future-hostile-language-rule`
   Reason: `Code shape and semantic surface` scored 45 below the standard floor of 85
   Fix: split large or ambiguous authored code into smaller semantic modules with focused tests
   Rerun: `just fast`
   Fingerprint: `sha256:a6799ba6d33539eab601c52c4e0fafda7561a24223c675844d45c55e922c7cf2`
   Evidence: largest authored code file: crates/sql/src/exec/mod.rs (1982 LOC), code file exceeds 500 LOC, code file exceeds 1000 LOC, most code files stay under 300 LOC
2. `medium` `security` `.github/workflows/jankurai.yml`
   Rule: `HLT-016-SUPPLY-CHAIN-DRIFT`
   Check: `HLT-016-SUPPLY-CHAIN-DRIFT:security` `soft` confidence `0.76`
   Route: TLR `Security, secrets, agency`, lane `security`, owner `ops`
   Docs: `docs/audit-rubric.md#top-level-risk-mapping`
   Reason: `Security and supply-chain posture` scored 70 below the standard floor of 85
   Fix: wire secret, dependency, provenance, and workflow scans into an operational CI lane
   Rerun: `just security`
   Fingerprint: `sha256:9a1bcfb2380532658b7b977985f45f9847a29fcbe2a9166297d7ba7f558bf3b4`
   Evidence: lockfile present, secret or dependency scan tooling found, security lane present, canonical security lane wrapper present
3. `medium` `proof` `Justfile`
   Rule: `HLT-018-PERF-CONCURRENCY-DRIFT`
   Check: `HLT-018-PERF-CONCURRENCY-DRIFT:proof` `soft` confidence `0.76`
   Route: TLR `Verification`, lane `fast`, owner `workspace`
   Docs: `docs/testing.md`
   Reason: `Build speed signals` scored 70 below the standard floor of 85
   Fix: add fast deterministic build/test targets, caches, and narrow proof lanes for agent iteration
   Rerun: `just fast`
   Fingerprint: `sha256:a256a7390d4b91a5b0a95d6f092e524c8f4080f27fe2b62e28cf0801343d0fef`
   Evidence: build acceleration markers found, targeted test/build commands found, locked dependency graph present, CI cache hint found
4. `medium` `copy-code` `crates/bench/src/fuzz/normalize.rs:19`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `Cell` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `Cell` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:6d9e620f810b773f768b6cd0b0b0cf5571d4c6453864b75648a06f6030310eb8`
   Evidence: enum `Cell` is defined with diverging shapes in 2 modules (crates/bench/src/fuzz/normalize.rs:19, crates/cli/src/render.rs:11)
5. `medium` `copy-code` `crates/kernel/src/error.rs:21`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `Error` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `Error` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:0c9d695968a4c3656809617dc18f78a9c390dfb8bf704adb486db7cd6f8a193f`
   Evidence: enum `Error` is defined with diverging shapes in 2 modules (crates/kernel/src/error.rs:21, crates/sql/src/error.rs:5)
6. `medium` `copy-code` `crates/kernel/src/json/path_bytecode.rs:27`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `Op` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `Op` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:e2e551967fd04b434513fb5ea4df0e967f24fefc1ec196da23480ead865c63ba`
   Evidence: enum `Op` is defined with diverging shapes in 2 modules (crates/kernel/src/json/path_bytecode.rs:27, crates/sql/src/exec/expr/program.rs:313)
7. `medium` `copy-code` `crates/redlinedb/src/iter.rs:13`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `Step` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `Step` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:e297ba16da13c28db635cdc0fb6c48ce85369732c19dfae28233d697f9921556`
   Evidence: enum `Step` is defined with diverging shapes in 2 modules (crates/redlinedb/src/iter.rs:13, crates/sql/src/statement.rs:893)
8. `medium` `copy-code` `crates/sql/src/exec/morsel/hash_agg.rs:44`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `AggKind` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `AggKind` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:f2af4a546b835a99095683cec5bb87eee646d8fbd2a7adeae90d751ec24e79a7`
   Evidence: enum `AggKind` is defined with diverging shapes in 2 modules (crates/sql/src/exec/morsel/hash_agg.rs:44, crates/sql/src/exec/vec/hash_agg.rs:23)
9. `medium` `copy-code` `crates/sql/src/planner.rs:113`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `JoinKind` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `JoinKind` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:4ffc8e6085d9d03576302f828516f68b55da913519dd57efc8e67d9c2b9bf817`
   Evidence: enum `JoinKind` is defined with diverging shapes in 2 modules (crates/sql/src/planner.rs:113, crates/sql/src/statement.rs:627)
10. `medium` `copy-code` `crates/sql/src/planner.rs:121`
   Rule: `HLT-046-UNNECESSARY-VARIETY`
   Check: `HLT-046-UNNECESSARY-VARIETY:copy-code` `soft` confidence `0.88`
   Route: TLR `Maintainability entropy`, lane `copy-code`, owner `tools`
   Docs: `agent/JANKURAI_STANDARD.md#jankurai-pillar-variety-and-canonical-shape`
   Matched term: `unnecessary-variety`
   Reason: enum `AccessPath` has 2 divergent definitions across modules where one consistent definition is expected
   Fix: define `AccessPath` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Rerun: `cargo run -p jankurai -- copy-code . --json target/jankurai/copy-code.json --md target/jankurai/copy-code.md`
   Fingerprint: `sha256:5a9766da6340b50c0c0c4141bb6f40b8414c7f581b21145ee2eefa32b88aec03`
   Evidence: enum `AccessPath` is defined with diverging shapes in 2 modules (crates/sql/src/planner.rs:121, crates/sql/src/planner/access_path.rs:105)

## Policy

- Policy file: `./agent/audit-policy.toml`
- Minimum score: `85`
- Fail on: `critical, high`

## Agent Fix Queue

1. `medium` `HLT-018-PERF-CONCURRENCY-DRIFT` `Justfile` - add fast deterministic build/test targets, caches, and narrow proof lanes for agent iteration
   Route: `Verification`/`fast`
2. `medium` `HLT-001-DEAD-MARKER` `.` - split large or ambiguous authored code into smaller semantic modules with focused tests
   Route: `Entropy`/`fast`
3. `medium` `HLT-016-SUPPLY-CHAIN-DRIFT` `.github/workflows/jankurai.yml` - wire secret, dependency, provenance, and workflow scans into an operational CI lane
   Route: `Security, secrets, agency`/`security`
4. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/bench/src/fuzz/normalize.rs` - define `Cell` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
5. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/kernel/src/error.rs` - define `Error` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
6. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/kernel/src/json/path_bytecode.rs` - define `Op` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
7. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/redlinedb/src/iter.rs` - define `Step` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
8. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/sql/src/exec/morsel/hash_agg.rs` - define `AggKind` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
9. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/sql/src/planner.rs` - define `JoinKind` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
10. `medium` `HLT-046-UNNECESSARY-VARIETY` `crates/sql/src/planner.rs` - define `AccessPath` once in a shared module and import it everywhere, or reconcile the diverging definitions so one canonical shape is used; redundant variety lets the copies drift apart
   Route: `Maintainability entropy`/`copy-code`
