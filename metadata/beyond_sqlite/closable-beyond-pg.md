# Closable beyond-Postgres cases

Work list for the in-scope beyond-SQLite corpus, regenerated against a live
PostgreSQL 16.15 oracle. Replaces `target/redline-testing/closable-beyond-pg.txt`,
which `docs/beyond-postgres-skips.md` cites as authoritative but which lived under
gitignored `target/` and no longer exists.

  corpus                          265 cases
  deferred (skip-list.toml)       117 cases
  in scope                        148 cases
  oracle self-compare usable      148/148 (100%)
  RedlineDB passes                123/148 (83.1%)
  remaining                       25 cases, listed below

2026-09-22 rerun against Postgres 16.15 locale C: `20029` (bytea hex) and
`20058` (ILIKE, ASCII-only fold) passed and left
`postgres-regression.json`. `20029` is removed from the list below.
`20058` was not in this 25-case triage.

## By root cause

-  9  error-classification
-  4  output-rendering
-  3  unsupported-statement
-  2  named-collations
-  2  parser
-  1  lateral-join
-  1  unsupported-function
-  1  table-valued-functions
-  1  type-semantics
-  1  schema-namespacing

## Cases

### BEYOND_COLLATIONS_ILIKE

- `20042` COLLATE_C_SORT_ORDER  [named-collations]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 1: bind error: no such collation sequence: "C"
- `20060` ORDER_BY_COLLATE_NULLS_LAST  [named-collations]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 1: bind error: no such collation sequence: "en-x-icu"
### BEYOND_LISTEN_NOTIFY

- `20438` LISTEN_INVALID_CHANNEL_NAME_ALL  [unsupported-statement]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: statement not supported yet: LISTEN { channel: Ident { value: "AL
### BEYOND_MIGRATION_ERGONOMICS

- `20203` ALTER_COLUMN_SET_NOT_NULL  [error-classification]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 19: constraint violation: NOT NULL constraint failed: mig_set_notnull.name
- `20209` ALTER_COLUMN_ADD_IDENTITY  [error-classification]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 19: constraint violation: NOT NULL constraint failed: mig_add_identity.id
### BEYOND_PORTABILITY_SYNTAX

- `20103` MERGE_NOT_MATCHED_BY_SOURCE_PG17  [error-classification]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: MERGE WHEN NOT MATCHED BY SOURCE (PG17+) is not supported
- `20106` LATERAL_WITH_SET_RETURNING_FUNCTION  [lateral-join]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 1002: unsupported sql: only direct table scans are supported
- `20111` DISTINCT_ON_REQUIRES_ORDER_BY  [error-classification]
      exit-code mismatch: reference=3 target=0 target_stderr=
- `20117` WINDOW_NAMED_REUSE  [output-rendering]
      stdout mismatch: reference="1|10|30|15.0000000000000000\n1|20|30|15.0000000000000000\n2|5|5|5.0000000000000000\n" target="1|10|30|15.0\n1|20|30|15.0\n
- `20136` GENERATED_ALWAYS_REJECTS_EXPLICIT  [error-classification]
      exit-code mismatch: reference=3 target=0 target_stderr=
- `20137` GENERATED_BY_DEFAULT_IDENTITY  [output-rendering]
      stdout mismatch: reference="1|auto\n" target="101|auto\n" target_stderr=
- `20139` IDENTITY_RESTART_VALUE  [parser]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 1: parse error: sql parser error: Expected: ), found: INCREMENT at Line: 1, Column: 85
### BEYOND_REPLICATION_CDC

- `20418` LOGICAL_SLOT_REJECTS_WAL_LEVEL_REPLICA  [unsupported-function]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: unsupported function pg_create_logical_replication_slot
- `20429` LOGICAL_SLOT_PEEK_REQUIRES_SLOT  [table-valued-functions]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: table-valued functions are not supported
### BEYOND_RICH_TYPES

- `20005` DECIMAL_NO_LOSS_VS_REAL  [output-rendering]
      stdout mismatch: reference="t|t\n" target="1|0\n" target_stderr=
- `20010` TIMESTAMP_DIFF_INTERVAL  [type-semantics]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 20: datatype mismatch
- `20021` ENUM_REJECTS_BAD_VALUE  [error-classification]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: only DROP TABLE, DROP INDEX, DROP VIEW, DROP SCHEMA, and DROP SEQ
- `20023` DOMAIN_REJECTS_NEG  [unsupported-statement]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: statement not supported yet: DropDomain(DropDomain { if_exists: t
### BEYOND_SCHEMAS_SEQUENCES

- `20247` GENERATED_ALWAYS_AS_IDENTITY_REJECTS_EXPLICIT  [error-classification]
      exit-code mismatch: reference=3 target=0 target_stderr=
- `20249` SEQUENCE_OWNED_BY  [error-classification]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 1002: unsupported sql: unsupported DDL expression: Function(Function { name: ObjectName(
- `20255` SAME_NAMED_TABLES_DIFFERENT_SCHEMAS  [schema-namespacing]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 19: kernel error: object already exists
### BEYOND_STORED_PROCEDURES

- `20308` PLPGSQL_RAISE_EXCEPTION  [parser]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1: parse error: sql parser error: Expected: an SQL statement, found: DO at Line: 2, Colu
### BEYOND_VECTOR_ADVANCED_INDEXES

- `20340` VECTOR_EXTENSION_PROBE_UNAVAILABLE  [unsupported-statement]
      exit-code mismatch: reference=3 target=1 target_stderr=Error: 1002: unsupported sql: statement not supported yet: CreateExtension(CreateExtension { na
- `20357` INDEX_BTREE_NULLS_FIRST  [error-classification]
      exit-code mismatch: reference=0 target=1 target_stderr=Error: 1002: unsupported sql: unsupported use of NULLS FIRST

