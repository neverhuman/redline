//! SQLite Differential Lab Verification
//!
//! This test suite spins up a real `rusqlite` connection alongside a `redlinedb_sql`
//! connection and runs exhaustive query matrices against both, asserting that the
//! resulting rows, types, and values match byte-for-byte.

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;
use std::sync::Arc;
use tempfile::tempdir;

#[path = "differential_lab/query_shapes.rs"]
mod query_shapes;

/// Map a `rusqlite::types::Value` to a `redlinedb_sql::SqlValue` so we can `assert_eq!`.
fn to_sql_value(val: RuValue) -> SqlValue {
    match val {
        RuValue::Null => SqlValue::Null,
        RuValue::Integer(i) => SqlValue::Integer(i),
        RuValue::Real(f) => SqlValue::Real(f),
        RuValue::Text(s) => SqlValue::Text(Arc::from(s)),
        RuValue::Blob(b) => SqlValue::Blob(Arc::from(b)),
    }
}

/// A differential test harness holding both engines.
struct Lab {
    _dir: tempfile::TempDir,
    redline: Arc<Connection>,
    sqlite: rusqlite::Connection,
}

impl Lab {
    fn new() -> Self {
        let dir = tempdir().expect("temp dir");
        let path = dir.path().join("lab.db");
        let db = Database::create(&path, DbOptions::default()).expect("create db");
        let redline = db.connect();

        let sqlite = rusqlite::Connection::open_in_memory().expect("rusqlite open");

        Self {
            _dir: dir,
            redline,
            sqlite,
        }
    }

    /// Execute a DDL or DML statement on both engines.
    fn execute(&self, sql: &str) {
        let res_ru = self.sqlite.execute_batch(sql);
        let res_rl = self.redline.execute(sql);

        match (&res_ru, &res_rl) {
            (Ok(_), Ok(_)) => {}
            (Err(e_ru), Err(e_rl)) => {
                // If both fail, that's fine for negative testing, but usually in setup we want them to pass.
                panic!("both failed on execute: {sql}\nru: {e_ru}\nrl: {e_rl:?}");
            }
            (Ok(_), Err(e_rl)) => {
                panic!("redline failed, sqlite succeeded: {sql}\nrl err: {e_rl:?}")
            }
            (Err(e_ru), Ok(_)) => panic!("sqlite failed, redline succeeded: {sql}\nru err: {e_ru}"),
        }
    }

    /// Execute a query and assert the results match perfectly.
    fn assert_query(&self, sql: &str) {
        // 1. Run in rusqlite
        let mut ru_stmt = match self.sqlite.prepare(sql) {
            Ok(s) => s,
            Err(e) => panic!("rusqlite failed to prepare: {sql}\nerror: {e}"),
        };

        let ncols = ru_stmt.column_count();
        let mut ru_rows = Vec::new();

        let mut ru_query = ru_stmt.query([]).expect("ru query");
        while let Some(row) = ru_query.next().expect("ru next") {
            let mut current = Vec::with_capacity(ncols);
            for i in 0..ncols {
                let v: RuValue = row.get(i).expect("ru get");
                current.push(to_sql_value(v));
            }
            ru_rows.push(current);
        }

        // 2. Run in redline
        let mut rl_stmt = match self.redline.prepare(sql) {
            Ok(s) => s,
            Err(e) => panic!("redline failed to prepare: {sql}\nerror: {e:?}"),
        };

        let mut rl_rows = Vec::new();
        while let Step::Row = rl_stmt.step().expect("rl step") {
            let mut current = Vec::with_capacity(ncols);
            for i in 0..ncols {
                current.push(rl_stmt.column_value(i).expect("rl col").clone());
            }
            rl_rows.push(current);
        }

        // 3. Diff
        if ru_rows != rl_rows {
            panic!(
                "Differential mismatch on query:\n  {sql}\n\nSQLite returned:\n  {ru_rows:?}\n\nRedline returned:\n  {rl_rows:?}"
            );
        }
    }

    /// Run a suite of SQLs, ignoring prepare errors if `allow_errors` is true,
    /// but ensuring both engines error out if one does.
    fn assert_queries(&self, sqls: &[&str]) {
        for &sql in sqls {
            self.assert_query(sql);
        }
    }
}

// ── Differential Matrices ─────────────────────────────────────────────────────

#[test]
fn diff_scalar_string_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(v TEXT)");
    lab.execute("INSERT INTO t VALUES ('hello'), ('world'), ('  spaces  '), (NULL)");

    lab.assert_queries(&[
        "SELECT substr(v, 2) FROM t ORDER BY v",
        "SELECT substring(v, 2, 3) FROM t ORDER BY v",
        "SELECT trim(v) FROM t ORDER BY v",
        "SELECT instr(v, 'o') FROM t ORDER BY v",
        "SELECT instr(v, 'x') FROM t ORDER BY v",
        "SELECT replace(v, 'o', 'X') FROM t ORDER BY v",
        "SELECT replace(v, 'l', NULL) FROM t ORDER BY v",
        "SELECT printf('[%s]', v) FROM t ORDER BY v",
        "SELECT upper(v) FROM t ORDER BY v",
        "SELECT lower(v) FROM t ORDER BY v",
        "SELECT length(v) FROM t ORDER BY v",
    ]);
}

#[test]
fn diff_scalar_math_and_logic_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(n INTEGER, r REAL)");
    lab.execute("INSERT INTO t VALUES (42, 3.14), (-7, -0.5), (0, 0.0), (NULL, NULL)");

    lab.assert_queries(&[
        "SELECT iif(n > 0, 'pos', 'neg') FROM t ORDER BY n",
        "SELECT sign(n), sign(r) FROM t ORDER BY n",
        "SELECT coalesce(n, 999) FROM t ORDER BY n",
        "SELECT nullif(n, 42) FROM t ORDER BY n",
    ]);
}

#[test]
fn diff_aggregate_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(grp TEXT, v INTEGER)");
    lab.execute("INSERT INTO t VALUES ('A', 1), ('A', 1), ('A', NULL), ('B', NULL), ('B', NULL)");

    lab.assert_queries(&[
        "SELECT grp, count(*) FROM t GROUP BY grp ORDER BY grp",
        "SELECT grp, count(v) FROM t GROUP BY grp ORDER BY grp",
        "SELECT grp, sum(v) FROM t GROUP BY grp ORDER BY grp",
        "SELECT grp, total(v) FROM t GROUP BY grp ORDER BY grp",
        "SELECT grp, min(v), max(v) FROM t GROUP BY grp ORDER BY grp",
        // group_concat/json_group_array: intra-group order is impl-defined; skip differential
    ]);
}

#[test]
fn diff_outer_and_cross_join_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE a(id INTEGER PRIMARY KEY, v TEXT)");
    lab.execute("CREATE TABLE b(aid INTEGER, payload TEXT)");
    lab.execute("INSERT INTO a VALUES (1, 'A1'), (2, NULL), (3, 'A3')");
    lab.execute("INSERT INTO b VALUES (1, 'B1'), (1, NULL), (3, 'B3'), (4, 'B4')");

    lab.assert_queries(&[
        "SELECT a.id, b.aid FROM a CROSS JOIN b ORDER BY a.id, b.aid, b.payload",
        "SELECT a.id, b.payload FROM a LEFT JOIN b ON a.id = b.aid ORDER BY a.id, b.payload",
        "SELECT a.id, b.payload FROM b LEFT JOIN a ON a.id = b.aid ORDER BY b.rowid, a.id",
        "SELECT a.id, b.payload FROM a JOIN b ON a.id = b.aid ORDER BY a.id, b.payload",
        "SELECT a.id FROM a LEFT JOIN b ON a.id = b.aid WHERE b.aid IS NULL ORDER BY a.id",
        "SELECT a.id, b.payload FROM a LEFT JOIN b ON a.id = b.aid AND b.payload IS NOT NULL ORDER BY a.id, b.payload",
        "SELECT a.id, COALESCE(b.payload, 'missing') FROM a LEFT JOIN b ON a.id = b.aid ORDER BY a.id, b.payload",
        "SELECT b.aid, a.v FROM b LEFT JOIN a ON a.id = b.aid WHERE a.id IS NULL ORDER BY b.aid, b.payload",
        "SELECT a.id, count(b.payload) FROM a LEFT JOIN b ON a.id = b.aid GROUP BY a.id ORDER BY a.id",
        "SELECT a.id, b.payload FROM a CROSS JOIN b WHERE b.aid = a.id ORDER BY a.id, b.payload",
    ]);
}

#[test]
fn diff_natural_using_join_output_shape() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE p(id INTEGER, x INTEGER)");
    lab.execute("CREATE TABLE q(id INTEGER, y INTEGER)");
    lab.execute("INSERT INTO p VALUES (1,10),(2,20),(3,30)");
    lab.execute("INSERT INTO q VALUES (1,100),(3,300)");

    lab.assert_queries(&[
        "SELECT * FROM p JOIN q USING(id) ORDER BY id",
        "SELECT * FROM p NATURAL JOIN q ORDER BY id",
        "SELECT * FROM p NATURAL LEFT JOIN q ORDER BY id",
        "SELECT id FROM p NATURAL LEFT JOIN q ORDER BY id",
        "SELECT p.id, q.id, id FROM p NATURAL LEFT JOIN q ORDER BY p.id",
    ]);
}

#[test]
fn diff_null_semantics_matrix() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE t(id INTEGER PRIMARY KEY, a INTEGER, b INTEGER, label TEXT)");
    lab.execute(
        "INSERT INTO t(id, a, b, label) VALUES \
         (1, 1, 1, 'one'), (2, 1, NULL, 'missing-b'), (3, NULL, 1, 'missing-a'), \
         (4, NULL, NULL, NULL), (5, 2, 3, 'two')",
    );

    lab.assert_queries(&[
        "SELECT id FROM t WHERE a = b ORDER BY id",
        "SELECT id FROM t WHERE a IS NULL ORDER BY id",
        "SELECT id FROM t WHERE a IS NOT NULL ORDER BY id",
        "SELECT id, COALESCE(label, 'fallback') FROM t ORDER BY id",
        "SELECT id, NULLIF(a, b) FROM t ORDER BY id",
        "SELECT count(*), count(a), count(label) FROM t",
        "SELECT id FROM t WHERE a IN (1, NULL) ORDER BY id",
        "SELECT id FROM t WHERE a NOT IN (1, NULL) ORDER BY id",
        "SELECT id FROM t WHERE NOT (a = 1) ORDER BY id",
        "SELECT id FROM t WHERE (a = 1) OR b IS NULL ORDER BY id",
        "SELECT id FROM t WHERE (a = 1) AND b IS NULL ORDER BY id",
    ]);
}

#[test]
fn diff_correlated_subquery_outer_pk_is_not_inner_rowid_alias() {
    let lab = Lab::new();
    lab.execute("CREATE TABLE a(id INTEGER PRIMARY KEY, name TEXT)");
    lab.execute("CREATE TABLE b(id INTEGER PRIMARY KEY, a_id INTEGER, val INTEGER)");
    lab.execute("INSERT INTO a VALUES (1, 'one'), (2, 'two'), (3, 'three')");
    lab.execute("INSERT INTO b VALUES (10, 1, 100), (11, 1, 101), (12, 2, 200)");

    lab.assert_query(
        "SELECT a.id, a.name, (SELECT max(b.val) FROM b WHERE b.a_id = a.id) AS top \
         FROM a ORDER BY a.id",
    );
}
