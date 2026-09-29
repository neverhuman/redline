//! Differential parity tests for the JSON table-valued functions
//! `json_each` and `json_tree` (Workstream A8).
//!
//! Each test runs the same SQL against `rusqlite` (the SQLite oracle) and
//! `redlinedb_sql`, then asserts the two engines return identical rows.
//! We materialise both row sets first and compare structurally — both
//! engines should agree on every column SQLite documents
//! (`key, value, type, atom, id, parent, fullkey, path`).

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;
use std::sync::Arc;
use tempfile::tempdir;

#[path = "parity_json_table/errors_and_tree.rs"]
mod errors_and_tree;

fn to_sql_value(val: RuValue) -> SqlValue {
    match val {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(i) => SqlValue::Integer(i),
        RuValue::Real(f) => SqlValue::Real(f),
        RuValue::Text(s) => SqlValue::Text(Arc::from(s)),
        RuValue::Blob(b) => SqlValue::Blob(Arc::from(b)),
    }
}

struct Pair {
    _dir: tempfile::TempDir,
    redline: Arc<Connection>,
    sqlite: rusqlite::Connection,
}

#[derive(Debug, PartialEq, Eq)]
enum ErrorClass {
    JsonPath,
    MalformedJson,
}

#[derive(Debug, PartialEq)]
enum QueryOutcome {
    Rows(Vec<Vec<SqlValue>>),
    Error {
        class: ErrorClass,
        stable_fragment: &'static str,
    },
}

fn classify_error(message: &str) -> QueryOutcome {
    let lower = message.to_ascii_lowercase();
    if lower.contains("malformed json") {
        return QueryOutcome::Error {
            class: ErrorClass::MalformedJson,
            stable_fragment: "malformed JSON",
        };
    }
    if lower.contains("json path") {
        return QueryOutcome::Error {
            class: ErrorClass::JsonPath,
            stable_fragment: "JSON path",
        };
    }
    panic!("unexpected JSON table-valued function error: {message}");
}

impl Pair {
    fn new() -> Self {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("lab.db");
        let db = Database::create(&path, DbOptions::default()).expect("create");
        let redline = db.connect();
        let sqlite = rusqlite::Connection::open_in_memory().expect("rusqlite open");
        Pair {
            _dir: dir,
            redline,
            sqlite,
        }
    }

    fn execute_both(&self, sql: &str) {
        self.sqlite.execute_batch(sql).expect("sqlite exec");
        self.redline.execute(sql).expect("redline exec");
    }

    fn redline_rows(&self, sql: &str) -> Vec<Vec<SqlValue>> {
        let mut stmt = self.redline.prepare(sql).expect("redline prepare");
        let ncols = stmt.column_count();
        let mut rows = Vec::new();
        while let Step::Row = stmt.step().expect("redline step") {
            rows.push(
                (0..ncols)
                    .map(|i| stmt.column_value(i).expect("redline col").clone())
                    .collect(),
            );
        }
        rows
    }

    fn sqlite_rows(&self, sql: &str) -> Vec<Vec<SqlValue>> {
        let mut stmt = self.sqlite.prepare(sql).expect("sqlite prepare");
        let ncols = stmt.column_count();
        let mut sqlite_rows = Vec::new();
        let mut query = stmt.query([]).expect("sqlite query");
        while let Some(row) = query.next().expect("sqlite next") {
            sqlite_rows.push(
                (0..ncols)
                    .map(|i| to_sql_value(row.get::<usize, RuValue>(i).expect("sqlite get")))
                    .collect(),
            );
        }
        sqlite_rows
    }

    fn assert_parity(&self, sql: &str) {
        let rl = self.redline_rows(sql);
        let sl = self.sqlite_rows(sql);
        assert_eq!(rl, sl, "rows differ for: {sql}");
    }

    fn redline_outcome(&self, sql: &str) -> QueryOutcome {
        let mut stmt = match self.redline.prepare(sql) {
            Ok(stmt) => stmt,
            Err(err) => return classify_error(&format!("{err:?}")),
        };
        let ncols = stmt.column_count();
        let mut rows = Vec::new();
        loop {
            match stmt.step() {
                Ok(Step::Row) => rows.push(
                    (0..ncols)
                        .map(|i| stmt.column_value(i).expect("redline col").clone())
                        .collect(),
                ),
                Ok(Step::Done) => return QueryOutcome::Rows(rows),
                Err(err) => return classify_error(&format!("{err:?}")),
            }
        }
    }

    fn sqlite_outcome(&self, sql: &str) -> QueryOutcome {
        let mut stmt = match self.sqlite.prepare(sql) {
            Ok(stmt) => stmt,
            Err(err) => return classify_error(&err.to_string()),
        };
        let ncols = stmt.column_count();
        let mut query = match stmt.query([]) {
            Ok(query) => query,
            Err(err) => return classify_error(&err.to_string()),
        };
        let mut rows = Vec::new();
        loop {
            match query.next() {
                Ok(Some(row)) => rows.push(
                    (0..ncols)
                        .map(|i| to_sql_value(row.get::<usize, RuValue>(i).expect("sqlite get")))
                        .collect(),
                ),
                Ok(None) => return QueryOutcome::Rows(rows),
                Err(err) => return classify_error(&err.to_string()),
            }
        }
    }

    fn assert_rejects_with_same_error_class(
        &self,
        sql: &str,
        class: ErrorClass,
        stable_fragment: &'static str,
    ) {
        let redline = self.redline_outcome(sql);
        let sqlite = self.sqlite_outcome(sql);
        let expected = QueryOutcome::Error {
            class,
            stable_fragment,
        };
        assert_eq!(redline, expected, "redline outcome for {sql}");
        assert_eq!(sqlite, expected, "sqlite outcome for {sql}");
    }
}

#[test]
fn json_each_array_emits_index_per_element() {
    let pair = Pair::new();
    // Project the columns whose semantics are spec-stable across SQLite
    // versions; `id`/`parent` differ in numbering schemes across SQLite
    // builds so they are exercised via shape-only assertions below.
    pair.assert_parity(
        "SELECT key, value, type, atom, fullkey, path \
         FROM json_each('[10, 20, 30]') ORDER BY key",
    );
}

#[test]
fn json_each_object_emits_row_per_property() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT key, value, type, atom, fullkey, path \
         FROM json_each('{\"a\":1,\"b\":\"two\",\"c\":null}') ORDER BY key",
    );
}

#[test]
fn json_each_atom_emits_single_row() {
    let pair = Pair::new();
    pair.assert_parity("SELECT key, value, type, atom, fullkey, path FROM json_each('42')");
    pair.assert_parity("SELECT key, value, type, atom, fullkey, path FROM json_each('\"hello\"')");
    pair.assert_parity("SELECT key, value, type, atom, fullkey, path FROM json_each('null')");
}

#[test]
fn json_each_with_path_walks_subtree() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT key, value, type, atom \
         FROM json_each('{\"items\":[1,2,3]}', '$.items') ORDER BY key",
    );
}

#[test]
fn json_each_nested_object_only_emits_immediate_children() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT key, type, fullkey FROM json_each('{\"a\":{\"b\":1},\"c\":2}') ORDER BY key",
    );
}

#[test]
fn json_each_empty_array_emits_zero_rows() {
    let pair = Pair::new();
    pair.assert_parity("SELECT key, value FROM json_each('[]')");
}

#[test]
fn json_each_empty_object_emits_zero_rows() {
    let pair = Pair::new();
    pair.assert_parity("SELECT key, value FROM json_each('{}')");
}

#[test]
fn json_each_filter_by_type() {
    let pair = Pair::new();
    pair.assert_parity(
        "SELECT key, value FROM json_each('[1, \"x\", 2, \"y\", 3]') \
         WHERE type = 'integer' ORDER BY key",
    );
}

#[test]
fn json_each_with_missing_path_returns_zero_rows() {
    let pair = Pair::new();
    pair.assert_parity("SELECT key, value FROM json_each('{\"a\":1}', '$.missing')");
}

#[test]
fn json_each_join_with_user_table() {
    let pair = Pair::new();
    pair.execute_both("CREATE TABLE labels(k INTEGER PRIMARY KEY, label TEXT)");
    pair.execute_both("INSERT INTO labels VALUES (0, 'zero'), (1, 'one'), (2, 'two')");
    pair.assert_parity(
        "SELECT labels.label, j.value \
         FROM json_each('[100, 200, 300]') AS j \
         JOIN labels ON labels.k = j.key \
         ORDER BY j.key",
    );
}

#[test]
fn json_each_used_in_subquery_select() {
    let pair = Pair::new();
    // Wrap json_each as a derived count to prove it is usable in WHERE/SELECT
    // contexts via the standard CTE-style materialisation.
    pair.assert_parity("SELECT (SELECT count(*) FROM json_each('[1,2,3,4,5]')) AS n");
}
