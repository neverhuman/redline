//! Q5-07: trigger chains and trigger recursion, against SQLite (rusqlite).
//!
//! SQLite's rule (OP_Program with P5 set): while `recursive_triggers` is off,
//! a trigger does not fire while that same trigger is already running; every
//! other trigger fires, however deeply nested. With it on, triggers nest
//! until the trigger-depth limit, where the statement fails with "too many
//! levels of trigger recursion" and changes nothing. The SQLite oracle here
//! runs with its trigger-depth limit set to RedlineDB's cap, so the boundary
//! is compared exactly.

use std::sync::Arc;

use redlinedb_sql::{Connection, Database, DbOptions, SqlValue, Step};
use rusqlite::types::Value as RuValue;
use tempfile::tempdir;

/// RedlineDB's trigger nesting cap (`TRIGGER_DEPTH_CAP` in
/// crates/sql/src/exec/trigger.rs, documented in docs/sqlite-parity.md).
const CAP: i32 = 8;
const TOO_DEEP: &str = "too many levels of trigger recursion";

struct Lab {
    _dir: tempfile::TempDir,
    redline: Arc<Connection>,
    sqlite: rusqlite::Connection,
}

impl Lab {
    fn new() -> Self {
        let dir = tempdir().expect("temp dir");
        let db = Database::create(dir.path().join("chains.db"), DbOptions::default())
            .expect("create db");
        let sqlite = rusqlite::Connection::open_in_memory().expect("open sqlite");
        // SAFETY: `sqlite` is a live connection; sqlite3_limit only sets a
        // per-connection limit.
        let previous = unsafe {
            rusqlite::ffi::sqlite3_limit(
                sqlite.handle(),
                rusqlite::ffi::SQLITE_LIMIT_TRIGGER_DEPTH,
                CAP,
            )
        };
        assert!(previous >= CAP, "SQLite's own limit is {previous}");
        Self {
            _dir: dir,
            redline: db.connect(),
            sqlite,
        }
    }

    /// Run every statement on both engines; each must succeed.
    fn setup(&self, statements: &[&str]) {
        for sql in statements {
            self.sqlite
                .execute_batch(sql)
                .unwrap_or_else(|e| panic!("sqlite failed {sql:?}: {e}"));
            self.redline
                .execute(sql)
                .unwrap_or_else(|e| panic!("redline failed {sql:?}: {e:?}"));
        }
    }

    /// Run one statement on both engines and return each outcome.
    fn run(&self, sql: &str) -> (Result<(), String>, Result<(), String>) {
        let sqlite = self.sqlite.execute_batch(sql).map_err(|e| e.to_string());
        let redline = self
            .redline
            .execute(sql)
            .map(|_| ())
            .map_err(|e| e.to_string());
        (sqlite, redline)
    }

    /// Both engines succeed, or both fail with the recursion-limit error.
    fn run_same(&self, sql: &str) {
        match self.run(sql) {
            (Ok(()), Ok(())) => {}
            (Err(sqlite), Err(redline)) => {
                assert!(sqlite.contains(TOO_DEEP), "sqlite: {sqlite}");
                assert!(
                    redline.contains(TOO_DEEP),
                    "{sql:?}: redline failed with {redline:?}, sqlite with {sqlite:?}"
                );
            }
            (sqlite, redline) => panic!("{sql:?}: sqlite {sqlite:?}, redline {redline:?}"),
        }
    }

    fn assert_same(&self, query: &str) {
        let sqlite = query_sqlite(&self.sqlite, query);
        let redline = query_redline(&self.redline, query);
        assert_eq!(redline, sqlite, "{query}");
    }
}

fn query_sqlite(c: &rusqlite::Connection, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = c.prepare(sql).expect("prepare");
    let cols = stmt.column_count();
    let mut rows = stmt.query([]).expect("query");
    let mut out = Vec::new();
    while let Some(row) = rows.next().expect("next") {
        out.push(
            (0..cols)
                .map(|i| match row.get::<_, RuValue>(i).expect("get") {
                    RuValue::Null => SqlValue::Null,
                    RuValue::Integer(v) => SqlValue::Integer(v),
                    RuValue::Real(v) => SqlValue::Real(v),
                    RuValue::Text(v) => SqlValue::Text(v.into()),
                    RuValue::Blob(v) => SqlValue::Blob(v.into()),
                })
                .collect(),
        );
    }
    out
}

fn query_redline(c: &Arc<Connection>, sql: &str) -> Vec<Vec<SqlValue>> {
    let mut stmt = c
        .prepare(sql)
        .unwrap_or_else(|e| panic!("redline prepare failed for {sql:?}: {e:?}"));
    let cols = stmt.column_count();
    let mut out = Vec::new();
    while let Step::Row = stmt.step().expect("step") {
        out.push(
            (0..cols)
                .map(|i| stmt.column_value(i).expect("column").clone())
                .collect(),
        );
    }
    out
}

#[test]
fn distinct_trigger_chain_runs_with_recursion_off() {
    let lab = Lab::new();
    lab.setup(&[
        "PRAGMA recursive_triggers = OFF",
        "CREATE TABLE a(x)",
        "CREATE TABLE b(x)",
        "CREATE TABLE audit(x)",
        "CREATE TABLE counter(n)",
        "INSERT INTO counter VALUES (0)",
        "CREATE TRIGGER a_to_b AFTER INSERT ON a BEGIN INSERT INTO b VALUES (NEW.x); END",
        "CREATE TRIGGER b_to_audit AFTER INSERT ON b BEGIN INSERT INTO audit VALUES ('b' || NEW.x); END",
        "CREATE TRIGGER audit_count AFTER INSERT ON audit BEGIN UPDATE counter SET n = n + 1; END",
    ]);
    lab.run_same("INSERT INTO a VALUES (1)");
    lab.run_same("INSERT INTO a VALUES (2)");
    lab.assert_same("SELECT x FROM b ORDER BY x");
    lab.assert_same("SELECT x FROM audit ORDER BY x");
    lab.assert_same("SELECT n FROM counter");
    // UPDATE and DELETE triggers chain the same way.
    lab.setup(&[
        "CREATE TRIGGER a_update AFTER UPDATE ON a BEGIN DELETE FROM b WHERE x = OLD.x; END",
        "CREATE TRIGGER b_delete BEFORE DELETE ON b BEGIN INSERT INTO audit VALUES ('gone' || OLD.x); END",
    ]);
    lab.run_same("UPDATE a SET x = 10 WHERE x = 1");
    lab.assert_same("SELECT x FROM b ORDER BY x");
    lab.assert_same("SELECT x FROM audit ORDER BY x");
    lab.assert_same("SELECT n FROM counter");
}

#[test]
fn self_trigger_is_skipped_when_off_and_nests_when_on() {
    let lab = Lab::new();
    lab.setup(&[
        "CREATE TABLE t(n)",
        "CREATE TRIGGER t_again AFTER INSERT ON t WHEN NEW.n < 5 BEGIN INSERT INTO t VALUES (NEW.n + 1); END",
        // Indirect: p -> q -> p.
        "CREATE TABLE p(n)",
        "CREATE TABLE q(n)",
        "CREATE TRIGGER p_to_q AFTER INSERT ON p WHEN NEW.n < 5 BEGIN INSERT INTO q VALUES (NEW.n + 1); END",
        "CREATE TRIGGER q_to_p AFTER INSERT ON q WHEN NEW.n < 5 BEGIN INSERT INTO p VALUES (NEW.n + 1); END",
    ]);
    for mode in ["OFF", "ON"] {
        lab.setup(&[
            &format!("PRAGMA recursive_triggers = {mode}"),
            "DELETE FROM t",
            "DELETE FROM p",
            "DELETE FROM q",
        ]);
        lab.run_same("INSERT INTO t VALUES (1)");
        lab.run_same("INSERT INTO p VALUES (1)");
        lab.assert_same("SELECT n FROM t ORDER BY n");
        lab.assert_same("SELECT n FROM p ORDER BY n");
        lab.assert_same("SELECT n FROM q ORDER BY n");
    }
}

#[test]
fn instead_of_self_insert_does_not_recurse() {
    let lab = Lab::new();
    lab.setup(&[
        "CREATE TABLE t(x)",
        "CREATE VIEW v AS SELECT x FROM t",
        "CREATE TRIGGER v_insert INSTEAD OF INSERT ON v BEGIN \
         INSERT INTO t VALUES (NEW.x); INSERT INTO v VALUES (NEW.x + 1); END",
        "PRAGMA recursive_triggers = OFF",
    ]);
    // Off: the nested INSERT INTO v would fire v_insert again, which is
    // running, so it fires nothing.
    lab.run_same("INSERT INTO v VALUES (1)");
    lab.assert_same("SELECT x FROM t ORDER BY x");
    // On: it nests to the limit and fails, and the statement changes nothing.
    lab.setup(&["PRAGMA recursive_triggers = ON"]);
    let (sqlite, redline) = lab.run("INSERT INTO v VALUES (100)");
    assert!(
        sqlite.as_ref().is_err_and(|e| e.contains(TOO_DEEP)),
        "{sqlite:?}"
    );
    assert!(
        redline.as_ref().is_err_and(|e| e.contains(TOO_DEEP)),
        "{redline:?}"
    );
    lab.assert_same("SELECT x FROM t ORDER BY x");
}

#[test]
fn trigger_depth_cap_boundary() {
    let lab = Lab::new();
    lab.setup(&["PRAGMA recursive_triggers = ON"]);
    // A trigger whose WHEN stops at `limit` runs `limit` nested frames for
    // INSERT 1: exactly the cap succeeds, one more fails. One table and
    // trigger per limit (RedlineDB cannot yet re-create a dropped trigger's
    // name).
    for limit in [CAP - 1, CAP, CAP + 1, CAP + 5] {
        let t = format!("t{limit}");
        lab.setup(&[
            &format!("CREATE TABLE {t}(n)"),
            &format!(
                "CREATE TRIGGER deeper{limit} AFTER INSERT ON {t} WHEN NEW.n < {limit} BEGIN \
                 INSERT INTO {t} VALUES (NEW.n + 1); END"
            ),
        ]);
        let (sqlite, redline) = lab.run(&format!("INSERT INTO {t} VALUES (1)"));
        if limit <= CAP {
            assert_eq!((&sqlite, &redline), (&Ok(()), &Ok(())), "limit {limit}");
        } else {
            assert!(
                sqlite.as_ref().is_err_and(|e| e.contains(TOO_DEEP)),
                "{sqlite:?}"
            );
            assert!(
                redline.as_ref().is_err_and(|e| e.contains(TOO_DEEP)),
                "limit {limit}: {redline:?}"
            );
        }
        lab.assert_same(&format!("SELECT count(*), coalesce(max(n), 0) FROM {t}"));
    }
}

#[test]
fn failing_body_leaves_no_partial_effects() {
    let lab = Lab::new();
    lab.setup(&[
        "CREATE TABLE t(x)",
        "CREATE TABLE log(x)",
        "CREATE TABLE audit(x)",
        "CREATE TABLE strict(x NOT NULL)",
        "CREATE TRIGGER t_log AFTER INSERT ON t BEGIN INSERT INTO log VALUES (NEW.x); END",
        // Nested one level down: the chain fails in its second trigger.
        "CREATE TRIGGER log_audit AFTER INSERT ON log BEGIN \
         INSERT INTO audit VALUES (NEW.x); INSERT INTO strict VALUES (nullif(NEW.x, 2)); END",
    ]);
    lab.run_same("INSERT INTO t VALUES (1)");
    let (sqlite, redline) = lab.run("INSERT INTO t VALUES (2)");
    assert!(sqlite.is_err(), "sqlite accepted a NOT NULL violation");
    assert!(redline.is_err(), "redline accepted a NOT NULL violation");
    for table in ["t", "log", "audit", "strict"] {
        lab.assert_same(&format!("SELECT x FROM {table} ORDER BY x"));
    }
    // A failed statement leaves no trigger running: with recursion off, the
    // same chain fires in full on the next statement.
    lab.setup(&["PRAGMA recursive_triggers = OFF"]);
    lab.run_same("INSERT INTO t VALUES (0)");
    for table in ["t", "log", "audit", "strict"] {
        lab.assert_same(&format!("SELECT x FROM {table} ORDER BY x"));
    }
}

#[test]
fn a_recursion_limit_error_leaves_no_trigger_running() {
    let lab = Lab::new();
    lab.setup(&[
        "PRAGMA recursive_triggers = ON",
        "CREATE TABLE t(n)",
        "CREATE TABLE seen(n)",
        "CREATE TRIGGER forever AFTER INSERT ON t BEGIN INSERT INTO t VALUES (NEW.n + 1); END",
    ]);
    let (sqlite, redline) = lab.run("INSERT INTO t VALUES (1)");
    assert!(
        sqlite.is_err() && redline.is_err(),
        "{sqlite:?} {redline:?}"
    );
    lab.assert_same("SELECT count(*) FROM t");
    // After the failure the next statements start from depth zero and see
    // no trigger as already running: with recursion off, `forever` fires
    // once for this insert (it is not running yet), and the p -> q -> p
    // chain fires p_to_q, q_to_p, and stops at the running p_to_q.
    lab.setup(&[
        "PRAGMA recursive_triggers = OFF",
        "CREATE TABLE p(n)",
        "CREATE TABLE q(n)",
        "CREATE TRIGGER p_to_q AFTER INSERT ON p BEGIN INSERT INTO q VALUES (NEW.n + 1); END",
        "CREATE TRIGGER q_to_p AFTER INSERT ON q BEGIN INSERT INTO p VALUES (NEW.n + 1); END",
    ]);
    lab.run_same("INSERT INTO t VALUES (1)");
    lab.run_same("INSERT INTO p VALUES (1)");
    for table in ["t", "p", "q", "seen"] {
        lab.assert_same(&format!("SELECT n FROM {table} ORDER BY n"));
    }
}
