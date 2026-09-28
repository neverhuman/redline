//! High-level SQLite parity traceability tests.
//!
//! This suite is intentionally small. It inspects the bundled `rusqlite`
//! reference build, checks representative passing behavior against that
//! reference, and keeps major full-SQLite gaps executable until they are
//! implemented. It does not write local parity evidence artifacts; official
//! SQLite parity evidence comes only from the pinned `neverhuman/redline-testing`
//! release artifact.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;
use std::collections::BTreeSet;
use std::fs;
use std::sync::Arc;
use tempfile::tempdir;

/// Rows compared with the bundled SQLite in
/// `reference_build_pragma_rows_match_for_supported_surfaces`.
const ROW_COMPARED_REFERENCE_PRAGMAS: &[&str] = &[
    "application_id",
    "cache_size",
    "database_list",
    "foreign_key_list",
    "foreign_keys",
    "index_info",
    "index_list",
    "integrity_check",
    "journal_mode",
    "query_only",
    "quick_check",
    "recursive_triggers",
    "synchronous",
    "table_info",
    "table_list",
    "table_xinfo",
    "temp_store",
    "user_version",
];

/// SQLite supports these and RedlineDB rejects them or answers differently;
/// `known_gap_pragmas_are_rejected_or_diverge` shows each gap.
const KNOWN_GAP_REFERENCE_PRAGMAS: &[&str] = &[
    "auto_vacuum",
    "cache_spill",
    "case_sensitive_like",
    "collation_list",
    "compile_options",
    "function_list",
    "index_xinfo",
    "module_list",
    "pragma_list",
    "schema_version",
    "trusted_schema",
    "wal_checkpoint",
];

/// RedlineDB accepts these and answers with its own stored or constant
/// value, which is not compared with SQLite (readback only).
const ACCEPTED_UNCOMPARED_REFERENCE_PRAGMAS: &[&str] = &[
    "analysis_limit",
    "automatic_index",
    "busy_timeout",
    "checkpoint_fullfsync",
    "data_version",
    "defer_foreign_keys",
    "encoding",
    "foreign_key_check",
    "freelist_count",
    "fullfsync",
    "hard_heap_limit",
    "ignore_check_constraints",
    "legacy_alter_table",
    "locking_mode",
    "max_page_count",
    "mmap_size",
    "page_count",
    "page_size",
    "reverse_unordered_selects",
    "secure_delete",
    "soft_heap_limit",
    "threads",
    "writable_schema",
];

/// RedlineDB rejects these with `PRAGMA <name> is not supported`;
/// `explicit_reject_pragmas_are_rejected_by_redline` checks each.
const EXPLICIT_REJECT_REFERENCE_PRAGMAS: &[&str] = &[
    "cell_size_check",
    "count_changes",
    "default_cache_size",
    "empty_result_callbacks",
    "full_column_names",
    "incremental_vacuum",
    "journal_size_limit",
    "optimize",
    "read_uncommitted",
    "short_column_names",
    "shrink_memory",
    "temp_store_directory",
    "wal_autocheckpoint",
];

fn sqlite_pragma_list(conn: &rusqlite::Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("PRAGMA pragma_list")
        .expect("prepare pragma_list");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("pragma_list rows");
    let mut out = rows
        .collect::<Result<Vec<_>, _>>()
        .expect("pragma_list values");
    out.sort();
    out
}

fn sqlite_reference_metadata() -> (String, Vec<String>, Vec<String>) {
    let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
    let version: String = conn
        .query_row("SELECT sqlite_version()", [], |row| row.get(0))
        .expect("sqlite version");
    let compile_options: Vec<String> = {
        let mut stmt = conn
            .prepare("PRAGMA compile_options")
            .expect("compile options");
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("compile option rows");
        rows.collect::<Result<Vec<_>, _>>()
            .expect("compile option values")
    };
    let pragmas = sqlite_pragma_list(&conn);
    (version, compile_options, pragmas)
}

fn classified_reference_pragmas() -> BTreeSet<String> {
    ROW_COMPARED_REFERENCE_PRAGMAS
        .iter()
        .chain(KNOWN_GAP_REFERENCE_PRAGMAS)
        .chain(ACCEPTED_UNCOMPARED_REFERENCE_PRAGMAS)
        .chain(EXPLICIT_REJECT_REFERENCE_PRAGMAS)
        .map(|name| (*name).to_owned())
        .collect()
}

struct Harness {
    _dir: tempfile::TempDir,
    redline: Arc<Connection>,
    sqlite: rusqlite::Connection,
}

impl Harness {
    fn new() -> Self {
        let dir = tempdir().expect("temp dir");
        let db = Database::create(dir.path().join("parity.db"), DbOptions::default())
            .expect("create redline db");
        let sqlite = rusqlite::Connection::open_in_memory().expect("open sqlite");
        Self {
            _dir: dir,
            redline: db.connect(),
            sqlite,
        }
    }

    fn execute_both(&self, sql: &str) {
        self.sqlite
            .execute_batch(sql)
            .unwrap_or_else(|err| panic!("sqlite setup failed for {sql:?}: {err}"));
        self.redline
            .execute(sql)
            .unwrap_or_else(|err| panic!("redline setup failed for {sql:?}: {err:?}"));
    }

    fn assert_query_matches(&self, sql: &str) {
        let sqlite_rows = query_sqlite(&self.sqlite, sql);
        let redline_rows = query_redline(&self.redline, sql);
        assert_eq!(
            redline_rows, sqlite_rows,
            "query mismatch for {sql:?}\nsqlite={sqlite_rows:?}\nredline={redline_rows:?}"
        );
    }

    /// SQLite accepts `sql` after `setup`, and RedlineDB rejects it with an
    /// error naming `fragment`. `setup` must succeed on both engines: a setup
    /// failure is a broken fixture, never a pass.
    fn assert_redline_rejects(&self, setup: &[&str], sql: &str, fragment: &str) {
        for stmt in setup {
            self.execute_both(stmt);
        }
        query_sqlite(&self.sqlite, sql);
        let err = match redline_accepts(&self.redline, sql) {
            Ok(()) => panic!("redline accepted {sql:?}, which this suite lists as rejected"),
            Err(err) => err.to_string(),
        };
        assert!(
            err.contains(fragment),
            "redline rejected {sql:?} with {err:?}, which does not name {fragment:?}"
        );
    }

    /// After `setup` (which must succeed on both engines) both engines run
    /// `sql` and return different rows. An error on either side is a broken
    /// fixture, not a divergence.
    fn assert_result_diverges(&self, setup: &[&str], sql: &str) {
        for stmt in setup {
            self.execute_both(stmt);
        }
        let sqlite_rows = query_sqlite(&self.sqlite, sql);
        let redline_rows = query_redline(&self.redline, sql);
        assert_ne!(
            redline_rows, sqlite_rows,
            "redline now matches SQLite for {sql:?}; move it out of the known gaps"
        );
    }
}

fn query_sqlite(conn: &rusqlite::Connection, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = conn
        .prepare(sql)
        .unwrap_or_else(|err| panic!("sqlite prepare failed for {sql:?}: {err}"));
    let columns = stmt.column_count();
    let mut rows = stmt.query([]).expect("sqlite query");
    let mut out = Vec::new();
    while let Some(row) = rows.next().expect("sqlite next") {
        let mut values = Vec::with_capacity(columns);
        for idx in 0..columns {
            let value: RuValue = row.get(idx).expect("sqlite value");
            values.push(to_sql_value(value));
        }
        out.push(values);
    }
    out
}

fn query_redline(conn: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    try_query_redline(conn, sql).unwrap_or_else(|err| {
        panic!("redline query failed for {sql:?}: {err:?}");
    })
}

fn try_query_redline(
    conn: &Arc<Connection>,
    sql: &str,
) -> Result<Vec<Vec<SqlValue>>, redlinedb_sql::Error> {
    let mut stmt = conn.prepare(sql)?;
    let columns = stmt.column_count();
    let mut out = Vec::new();
    while let Step::Row = stmt.step()? {
        let mut values = Vec::with_capacity(columns);
        for idx in 0..columns {
            values.push(stmt.column_value(idx)?.clone());
        }
        out.push(values);
    }
    Ok(out)
}

fn redline_accepts(conn: &Arc<Connection>, sql: &str) -> Result<(), redlinedb_sql::Error> {
    let trimmed = sql.trim_start();
    let upper = trimmed.to_ascii_uppercase();
    if upper.starts_with("SELECT") || upper.starts_with("WITH") {
        let mut stmt = conn.prepare(sql)?;
        while let Step::Row = stmt.step()? {}
        return Ok(());
    }
    conn.execute(sql).map(|_| ())
}

fn to_sql_value(value: RuValue) -> SqlValue {
    match value {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(value) => SqlValue::Integer(value),
        RuValue::Real(value) => SqlValue::Real(value),
        RuValue::Text(value) => SqlValue::Text(Arc::from(value)),
        RuValue::Blob(value) => SqlValue::Blob(Arc::from(value)),
    }
}

#[test]
fn reference_build_metadata_is_available() {
    let (version, compile_options, pragmas) = sqlite_reference_metadata();

    println!("rusqlite_crate_version=0.37.0");
    println!("sqlite_version={version}");
    println!("compile_options:");
    for option in &compile_options {
        println!("{option}");
    }
    println!("pragma_list:");
    for pragma in &pragmas {
        println!("{pragma}");
    }

    assert!(!version.trim().is_empty(), "empty sqlite_version()");
    assert!(
        !compile_options.is_empty(),
        "bundled SQLite reported no compile options"
    );
    assert!(
        !pragmas.is_empty(),
        "bundled SQLite reported no PRAGMA names"
    );
}

#[test]
fn reference_build_pragmas_are_fully_classified() {
    let (_, _, pragmas) = sqlite_reference_metadata();
    let reference: BTreeSet<String> = pragmas.into_iter().collect();
    let classified = classified_reference_pragmas();

    let missing: Vec<_> = reference.difference(&classified).cloned().collect();
    let retired: Vec<_> = classified.difference(&reference).cloned().collect();

    assert!(
        missing.is_empty(),
        "bundled SQLite exposes unclassified PRAGMA(s): {missing:?}"
    );
    assert!(
        retired.is_empty(),
        "PRAGMA classification contains names not emitted by bundled SQLite: {retired:?}"
    );
}

#[test]
fn reference_build_pragma_rows_match_for_supported_surfaces() {
    let harness = Harness::new();
    harness.execute_both("PRAGMA foreign_keys = ON");
    harness.execute_both("PRAGMA recursive_triggers = ON");
    harness.execute_both("PRAGMA user_version = 7");
    harness.execute_both("PRAGMA journal_mode = memory");
    harness.execute_both("PRAGMA synchronous = FULL");
    harness.execute_both("PRAGMA temp_store = MEMORY");
    harness.execute_both("PRAGMA cache_size = -256");
    harness.execute_both("PRAGMA query_only = OFF");
    harness.execute_both("PRAGMA application_id = 42");
    harness.execute_both("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT, n INTEGER)");
    harness.execute_both("CREATE TABLE x(a TEXT, b INTEGER)");
    harness.execute_both("CREATE TABLE parent(id INTEGER PRIMARY KEY, label TEXT)");
    harness.execute_both(
        "CREATE TABLE child(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES parent(id), label TEXT)",
    );
    harness.execute_both("CREATE INDEX t_name_idx ON t(name)");
    harness.execute_both(
        "INSERT INTO t(id, name, n) VALUES (1, ' Ada ', 10), (2, 'Grace', NULL), (3, NULL, 5)",
    );
    harness.execute_both("INSERT INTO parent(id, label) VALUES (1, 'Ada'), (2, 'Grace')");
    harness.execute_both(
        "INSERT INTO child(id, parent_id, label) VALUES (10, 1, 'alpha'), (11, NULL, 'beta')",
    );

    let compared = [
        "PRAGMA foreign_keys",
        "PRAGMA recursive_triggers",
        "PRAGMA user_version",
        "PRAGMA journal_mode",
        "PRAGMA synchronous",
        "PRAGMA temp_store",
        "PRAGMA cache_size",
        "PRAGMA query_only",
        "PRAGMA integrity_check",
        "PRAGMA quick_check",
        "PRAGMA application_id",
        "SELECT seq, name FROM pragma_database_list() WHERE name = 'main' ORDER BY seq",
        "SELECT cid, name, type, dflt_value, pk FROM pragma_table_info('t') ORDER BY cid",
        "PRAGMA table_xinfo('x')",
        "SELECT name, \"unique\", origin FROM pragma_index_list('t') \
         WHERE name NOT LIKE 'sqlite_autoindex_%' ORDER BY name",
        "SELECT seqno, cid, name FROM pragma_index_info('t_name_idx') ORDER BY seqno",
        "SELECT id, seq, \"table\", \"from\", \"to\", on_update, on_delete, \"match\" \
         FROM pragma_foreign_key_list('child') ORDER BY id, seq",
        "SELECT schema, name, type, ncol, wr, strict FROM pragma_table_list \
         WHERE schema = 'main' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    ];
    for sql in compared {
        harness.assert_query_matches(sql);
    }

    let sqlite_pragmas = sqlite_pragma_list(&harness.sqlite);
    for pragma in ROW_COMPARED_REFERENCE_PRAGMAS {
        assert!(
            sqlite_pragmas.iter().any(|name| name == pragma),
            "row-compared PRAGMA {pragma} is not present in bundled SQLite pragma_list"
        );
        assert!(
            compared.iter().any(|sql| sql.contains(pragma)),
            "row-compared PRAGMA {pragma} is not compared by this test"
        );
    }
}

/// Every PRAGMA this suite lists as an explicit reject is rejected by
/// RedlineDB with a message naming it, while SQLite accepts its read form.
#[test]
fn explicit_reject_pragmas_are_rejected_by_redline() {
    for name in EXPLICIT_REJECT_REFERENCE_PRAGMAS {
        let harness = Harness::new();
        harness.assert_redline_rejects(
            &[],
            &format!("PRAGMA {name}"),
            &format!("PRAGMA {name} is not supported"),
        );
    }
}

/// RedlineDB accepts the read form of these PRAGMAs and answers with its own
/// stored or constant value. No SQLite value is claimed for them.
#[test]
fn accepted_uncompared_pragmas_are_accepted_by_redline() {
    for name in ACCEPTED_UNCOMPARED_REFERENCE_PRAGMAS {
        let harness = Harness::new();
        let sql = format!("PRAGMA {name}");
        query_sqlite(&harness.sqlite, &sql);
        try_query_redline(&harness.redline, &sql)
            .unwrap_or_else(|err| panic!("redline rejected accepted PRAGMA {name}: {err}"));
    }
}

/// Each known-gap PRAGMA has a fixture that shows the gap: RedlineDB rejects
/// a PRAGMA SQLite supports, or both answer and the rows differ.
#[test]
fn known_gap_pragmas_are_rejected_or_diverge() {
    for name in KNOWN_GAP_REFERENCE_PRAGMAS {
        let harness = Harness::new();
        match *name {
            "collation_list" | "function_list" | "module_list" | "pragma_list" => harness
                .assert_redline_rejects(
                    &[],
                    &format!("PRAGMA {name}"),
                    &format!("PRAGMA {name} is not supported"),
                ),
            // RedlineDB lists its own build options.
            "compile_options" => harness.assert_result_diverges(&[], "PRAGMA compile_options"),
            // RedlineDB omits the rowid key row (`1|-1||0|BINARY|0`).
            "index_xinfo" => harness.assert_result_diverges(
                &[
                    "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)",
                    "CREATE INDEX t_name_idx ON t(name)",
                ],
                "PRAGMA index_xinfo('t_name_idx')",
            ),
            // RedlineDB has no SQLite WAL and answers (0, 0, 0).
            "wal_checkpoint" => harness.assert_result_diverges(
                &["PRAGMA journal_mode=WAL"],
                "PRAGMA wal_checkpoint(FULL)",
            ),
            // SQLite defaults to 1; RedlineDB answers 0.
            "trusted_schema" => harness.assert_result_diverges(&[], "PRAGMA trusted_schema"),
            // SQLite reports its computed spill threshold; RedlineDB echoes the setting.
            "cache_spill" => {
                harness.assert_result_diverges(&["PRAGMA cache_spill = 7"], "PRAGMA cache_spill")
            }
            // RedlineDB stores and echoes the setting and never vacuums;
            // SQLite ignores a change once the database has a table.
            "auto_vacuum" => harness.assert_result_diverges(
                &["CREATE TABLE t(x INTEGER)", "PRAGMA auto_vacuum = FULL"],
                "PRAGMA auto_vacuum",
            ),
            // RedlineDB counts schema changes differently.
            "schema_version" => harness.assert_result_diverges(
                &["CREATE TABLE a(x INTEGER)", "CREATE INDEX a_x ON a(x)"],
                "PRAGMA schema_version",
            ),
            // SQLite's read form returns no row; RedlineDB returns one.
            "case_sensitive_like" => {
                harness.assert_result_diverges(&[], "PRAGMA case_sensitive_like")
            }
            other => panic!("known-gap PRAGMA {other} has no fixture that shows the gap"),
        }
    }
}

#[test]
fn known_full_sqlite_parity_gaps_are_explicit_failures() {
    let harness = Harness::new();
    // Q5-10 closed this gap: ORDER BY uses the column's declared NOCASE.
    harness.execute_both("CREATE TABLE names(name TEXT COLLATE NOCASE)");
    harness.execute_both("INSERT INTO names(name) VALUES ('a'), ('B')");
    harness.assert_query_matches("SELECT name FROM names ORDER BY name");
}

#[test]
fn sqlite_native_file_format_is_not_compatibility_surface() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("redline-native.db");
    {
        let db = Database::create(&path, DbOptions::default()).expect("create redline db");
        let conn = db.connect();
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT)")
            .expect("create table");
        conn.execute("INSERT INTO t(id, name) VALUES (1, 'Ada')")
            .expect("insert");
    }

    assert!(
        path.is_dir(),
        "RedlineDB root should remain a directory, not a SQLite database file"
    );
    let mut entries = fs::read_dir(&path)
        .unwrap_or_else(|err| panic!("read RedlineDB root directory {}: {err}", path.display()));
    assert!(
        entries.next().is_some(),
        "RedlineDB root should contain Redline-native files"
    );

    let sqlite = rusqlite::Connection::open(&path);
    assert!(
        sqlite.is_err(),
        "SQLite should not open a RedlineDB-native directory as a valid SQLite database"
    );
}

#[test]
fn sqlite_native_file_is_not_redline_database_root() {
    let dir = tempdir().expect("temp dir");
    let path = dir.path().join("sqlite-native.db");
    {
        let sqlite = rusqlite::Connection::open(&path).expect("create sqlite file");
        sqlite
            .execute_batch(
                "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT);
                 INSERT INTO t(id, name) VALUES (1, 'Ada');",
            )
            .expect("seed sqlite file");
    }

    assert!(
        path.is_file(),
        "SQLite should create a native database file at {}",
        path.display()
    );
    let redline = Database::open(&path, DbOptions::default());
    assert!(
        redline.is_err(),
        "RedlineDB should not open a SQLite-native database file as a RedlineDB root"
    );
}
